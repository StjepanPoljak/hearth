/* std's print macros panic when the output is gone, as in `hearth | head`
 * once head exits: Rust ignores SIGPIPE, so the write fails with EPIPE.
 * Hearth must keep running the guest, and must not die of SIGPIPE either,
 * as a VNC or serial socket client may go away too. So these shadow std's
 * for the whole crate and drop what can't be written. */
macro_rules! println {
    ($($arg:tt)*) => {{ use std::io::Write; let _ = writeln!(std::io::stdout(), $($arg)*); }}
}
macro_rules! print {
    ($($arg:tt)*) => {{ use std::io::Write; let _ = write!(std::io::stdout(), $($arg)*); }}
}
macro_rules! eprintln {
    ($($arg:tt)*) => {{ use std::io::Write; let _ = writeln!(std::io::stderr(), $($arg)*); }}
}
#[allow(unused_macros)]
macro_rules! eprint {
    ($($arg:tt)*) => {{ use std::io::Write; let _ = write!(std::io::stderr(), $($arg)*); }}
}

#[allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]
mod kvm_bindings {
    include!(concat!(env!("OUT_DIR"), "/kvm-bindings.rs"));
}
use kvm_bindings::*;

include!("frontend/mod.rs");
include!("util/mod.rs");
include!("arch/mod.rs");
include!("virtio/mod.rs");

mod config;
use config::*;

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use clap::Parser;
use std::fs::File;
use std::io::{ self };
use libc::{ _IOW, _IO };
use std::collections::HashMap;
use std::sync::{ Arc,Mutex };
use std::sync::atomic::{ AtomicBool, Ordering };
use std::cell::Cell;

type MMIO = kvm_run__bindgen_ty_1__bindgen_ty_6;
type IO = kvm_run__bindgen_ty_1__bindgen_ty_4;
type MMIODevices = HashMap::<u64, Arc<Mutex<dyn MMIODevice>>>;
type IODevices = HashMap::<u64, Arc<Mutex<dyn IODevice>>>;
/* Shared, so a device reading guest RAM from a frontend thread keeps
 * its region mapped after the VM is gone. */
type GuestMemory = Vec<Arc<MemRegion>>;

pub trait MMIODevice: Send {
    fn handle(&mut self, mmio: &mut MMIO, mem: &GuestMemory) -> io::Result<()>;
    /* Picks up work that arrived from outside the guest, see VmControl::kick(). */
    fn poll(&mut self, _mem: &GuestMemory) -> io::Result<()> { Ok(()) }
}

pub trait IODevice: Send {
    fn handle(&mut self, io: &IO, base: *mut u8) -> io::Result<()>;
}

thread_local! {
    /* The kvm_run of the vcpu running on this thread, for kick_handler(). */
    static KVM_RUN_PTR: Cell<*mut kvm_run> = const { Cell::new(std::ptr::null_mut()) };
}

/* Per-VM state shared by its vcpu threads and device threads. */
pub struct VmControl {
    should_stop: AtomicBool,
    devices_pending: AtomicBool,
    /* Threads currently in VCPU::run(), the targets of kick_signal(). */
    vcpu_threads: Mutex<Vec<libc::pthread_t>>
}

impl VmControl {
    fn new() -> Self {
        Self {
            should_stop: AtomicBool::new(false),
            devices_pending: AtomicBool::new(false),
            vcpu_threads: Mutex::new(vec![])
        }
    }

    /* Makes every vcpu thread leave its run loop. */
    pub fn stop(&self) {
        self.should_stop.store(true, Ordering::SeqCst);
        for tid in self.vcpu_threads.lock().unwrap().iter() {
            unsafe { libc::pthread_kill(*tid, kick_signal()); }
        }
    }

    /* Called from device threads: makes a vcpu thread poll the MMIO devices. */
    pub fn kick(&self) {
        self.devices_pending.store(true, Ordering::SeqCst);
        if let Some(tid) = self.vcpu_threads.lock().unwrap().first() {
            unsafe { libc::pthread_kill(*tid, kick_signal()); }
        }
    }

    fn should_stop(&self) -> bool {
        self.should_stop.load(Ordering::SeqCst)
    }

    fn take_devices_pending(&self) -> bool {
        self.devices_pending.swap(false, Ordering::SeqCst)
    }

    /* Kicks only reach registered threads, and a thread is unregistered
     * under the same lock, so pthread_kill never sees a stale thread id. */
    fn register_vcpu_thread(&self) {
        self.vcpu_threads.lock().unwrap().push(unsafe { libc::pthread_self() });
    }

    fn unregister_vcpu_thread(&self) {
        let tid = unsafe { libc::pthread_self() };
        self.vcpu_threads.lock().unwrap().retain(|t| unsafe { libc::pthread_equal(*t, tid) } == 0);
    }
}

const KVM_CREATE_VM: libc::Ioctl = _IO(KVMIO, 0x01);
const KVM_CHECK_EXTENSION: libc::Ioctl = _IO(KVMIO, 0x03);
const KVM_CREATE_VCPU: libc::Ioctl = _IO(KVMIO, 0x41);
const KVM_GET_VCPU_MMAP_SIZE: libc::Ioctl = _IO(KVMIO, 0x04);

const KVM_SET_USER_MEMORY_REGION: libc::Ioctl = _IOW::<kvm_userspace_memory_region>(KVMIO, 0x46);
const KVM_GET_DIRTY_LOG: libc::Ioctl = _IOW::<kvm_dirty_log>(KVMIO, 0x42);
const KVM_REGISTER_COALESCED_MMIO: libc::Ioctl = _IOW::<kvm_coalesced_mmio_zone>(KVMIO, 0x67);
const PAGE_SIZE: u64 = 4096;
const KVM_RUN: libc::Ioctl = _IO(KVMIO, 0x80);

#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Args {
    /// Read the VM's settings from this TOML file: each flag --foo X is
    /// foo = X there, and --disk's settings go in a [disk] table. Flags
    /// given here override the file's settings
    #[arg(long, value_name = "FILE")]
    config: Option<String>,

    /// Binary to run
    #[arg(short, long)]
    binary: Option<String>,

    /// Kernel to boot: a vmlinux or other PVH ELF on x86, an Image on aarch64
    #[arg(long, alias = "kernel")]
    linux: Option<String>,

    /// Memory size in bytes, or with a K, M or G suffix, e.g. 512M; a
    /// multiple of the page size [default: 256K for --binary, 1G for --linux]
    #[arg(short, long, value_parser = parse_mem_size)]
    memory: Option<usize>,

    /// Load address [default: 0x1000]
    #[arg(short, long)]
    load_addr: Option<u64>,

    /// Device tree blob
    #[arg(long)]
    dtb: Option<String>,

    /// Initramfs, or on x86 any file to pass as a PVH module
    #[arg(long)]
    initramfs: Option<String>,

    /// Disk image, with optional virtio-blk settings after it:
    /// PATH[,queue_size=N][,seg_max=N]. queue_size is the request queue
    /// size, offered as QueueNumMax [default: 128]; split rings must be a
    /// power of 2, and Linux refuses other sizes. seg_max offers
    /// VIRTIO_BLK_F_SEG_MAX with this many data segments per request
    /// [default: not offered, the guest uses 1]. Without indirect
    /// descriptors a request takes seg_max + 2 ring entries, so keep
    /// seg_max <= queue_size - 2 or the guest can build requests that
    /// never fit
    #[arg(long, value_parser = parse_disk, value_name = "PATH[,KEY=VALUE...]")]
    disk: Option<DiskConfig>,

    /// Tap interface name
    #[arg(long)]
    tap: Option<String>,

    /// Extra kernel command line arguments (x86; on aarch64 the DTB has them)
    #[arg(long, allow_hyphen_values = true)]
    append: Option<String>,

    /// Serve the display and keyboard to VNC clients on this address,
    /// e.g. 127.0.0.1:5900. No authentication: keep it local or tunnel it.
    #[arg(long)]
    vnc: Option<String>,

    /// Where the serial console goes: stdio (this terminal; Ctrl-A x
    /// quits), pty (prints the device to open with screen or minicom),
    /// file:PATH (output only) or unix:PATH (a socket, one client at a time)
    /// [default: stdio]
    #[arg(long, value_parser = parse_serial)]
    serial: Option<SerialSpec>,

    /// Name of this VM: the process takes it (as ps and top show it, at
    /// most 15 bytes), so several VMs can be told apart
    #[arg(long)]
    name: Option<String>,

    /// Log the guest's writes to GPA..GPA+SIZE with KVM's dirty log and
    /// print the pages written when the VM stops, e.g. 0x200000 or
    /// 0x200000:64K [default SIZE: one page]
    #[arg(long, value_parser = parse_dirty_log)]
    dirty_log: Option<(u64, usize)>,

    /// Report each time the guest is about to execute this address, with
    /// its registers, and go on: a symbol of the --linux ELF, optionally
    /// +OFFSET, or an address, e.g. start_kernel or 0xffffffff81000000.
    /// Repeat for more, at most 4 (x86 only, hardware breakpoints)
    #[arg(long = "break", value_name = "SYMBOL[+OFFSET]|ADDRESS")]
    breakpoints: Vec<String>
}

impl Args {
    /* The --config file's settings, overridden by the flags given. */
    fn vm_config(&self) -> io::Result<VmConfig> {
        let mut config = match &self.config {
            Some(path) => VmConfig::load(path)?,
            None => VmConfig::default()
        };
        macro_rules! set {
            ($($field:ident),*) => {
                $(if let Some(value) = &self.$field {
                    config.$field = Some(value.clone());
                })*
            }
        }
        set!(name, binary, linux, memory, dtb, initramfs, append, disk, tap, vnc);
        if let Some(load_addr) = self.load_addr {
            config.load_addr = load_addr;
        }
        if let Some(serial) = &self.serial {
            config.serial = serial.clone();
        }
        config.validate().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        Ok(config)
    }
}

/* GPA[:SIZE], GPA in hex with 0x or decimal, SIZE as for --memory. */
fn parse_dirty_log(arg: &str) -> Result<(u64, usize), String> {
    let (gpa, size) = match arg.split_once(':') {
        Some((gpa, size)) => (gpa, parse_mem_size(size)?),
        None => (arg, PAGE_SIZE as usize)
    };
    let gpa = parse_number(gpa).map_err(|e| format!("invalid GPA: {}", e))?;
    Ok((gpa, size))
}

pub struct KvmDev {
    pub file: File,
    pub kvm_run_size: usize
}

impl KvmDev {
    fn new() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .expect("Failed to open /dev/kvm");

        // 140904 ioctl(3</dev/kvm<char 10:232>>, 0xae04 /* KVM_GET_VCPU_MMAP_SIZE */, 0) = 12288
        let kvm_run_size = unsafe { libc::ioctl(file.as_raw_fd(), KVM_GET_VCPU_MMAP_SIZE, 0usize) };
        if kvm_run_size < 0 {
            return Err(io::Error::last_os_error());
        }
        
        Ok(Self { file, kvm_run_size: kvm_run_size as usize })
    }

    fn create_vm(&self) -> io::Result<VM> {
        // 140900 ioctl(3</dev/kvm<char 10:232>>, 0xae01 /* KVM_CREATE_VM */, 0) = 9<anon_inode:kvm-vm>
        let vm_fd = unsafe { libc::ioctl(self.fd(), KVM_CREATE_VM, 0usize) };
        if vm_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(VM {
            fd: vm_fd,
            guest_memory: GuestMemory::new(),
            slots: vec![],
            next_slot: 0,
            mmio_devices: MMIODevices::new(),
            io_devices: IODevices::new(),
            display: None,
            keyboard: None,
            control: Arc::new(VmControl::new()) })
    }

    fn fd(&self) -> libc::c_int {
        self.file.as_raw_fd()
    }

    fn get_kvm_run_size(&self) -> usize {
        self.kvm_run_size
    }
}

pub struct VM {
    pub fd: libc::c_int,
    pub guest_memory: GuestMemory,
    /* The KVM slots mapping guest_memory: one per region, until
     * log_dirty_pages splits it. */
    slots: Vec<Slot>,
    next_slot: u32,
    pub mmio_devices: MMIODevices,
    pub io_devices: IODevices,
    /* Set by the devices that provide them, for frontends such as RFB. */
    pub display: Option<Arc<dyn Display>>,
    pub keyboard: Option<Arc<Mutex<dyn Keyboard>>>,
    pub control: Arc<VmControl>
}

pub struct MemRegion {
    pub mem_ptr: *mut libc::c_void,
    pub mem_size: usize,
    pub guest_phys_addr: u64,
    /* Parts of the region the VMM fills before the guest runs; see
     * add_bump_allocator. */
    pub allocators: Vec<BumpAllocator>
}

impl MemRegion {
    /* Adds an allocator handing out gpa_start..gpa_end, which must lie in
     * this region and clear of its other allocators. Returns its index in
     * allocators. */
    pub fn add_bump_allocator(&mut self, gpa_start: u64, gpa_end: u64) -> io::Result<usize> {
        let region_end = self.guest_phys_addr + self.mem_size as u64;
        if gpa_start > gpa_end || gpa_start < self.guest_phys_addr || gpa_end > region_end {
            return Err(io::Error::other(format!("Allocator {:#x}..{:#x} is not inside region {:#x}..{:#x}.",
                                                gpa_start, gpa_end, self.guest_phys_addr, region_end)));
        }
        if let Some(other) = self.allocators.iter().find(|a| gpa_start < a.gpa_end() && a.gpa_start() < gpa_end) {
            return Err(io::Error::other(format!("Allocator {:#x}..{:#x} overlaps {:#x}..{:#x}.",
                                                gpa_start, gpa_end, other.gpa_start(), other.gpa_end())));
        }
        let host_start = unsafe { (self.mem_ptr as *mut u8).add((gpa_start - self.guest_phys_addr) as usize) };
        self.allocators.push(BumpAllocator::new(gpa_start, gpa_end, host_start));
        Ok(self.allocators.len() - 1)
    }
}

struct Slot {
    id: u32,
    guest_phys_addr: u64,
    size: u64,
    userspace_addr: u64,
    logged: bool
}

/* Which pages of a memory slot the guest has written since the last
 * fetch. KVM write-protects the slot's pages and notes each one on its
 * first write (or, on Intel, the CPU logs it with PML). */
pub struct DirtyLog {
    vm_fd: libc::c_int,
    /* Of the first page; bit n is the page n pages after it. */
    pub gpa: u64,
    slot: u32,
    pages: usize
}

impl DirtyLog {
    /* A bit per page, set if the page was written; fetching clears the
     * log, so a write made after this returns is in the next one. */
    pub fn fetch(&self) -> io::Result<Vec<u64>> {
        /* KVM copies whole longs. */
        let mut bitmap = vec![0u64; self.pages.div_ceil(64)];
        let log = kvm_dirty_log {
            slot: self.slot,
            padding1: 0,
            __bindgen_anon_1: kvm_dirty_log__bindgen_ty_1 { dirty_bitmap: bitmap.as_mut_ptr().cast() }
        };
        let ret = unsafe { libc::ioctl(self.vm_fd, KVM_GET_DIRTY_LOG, &log) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(bitmap)
    }
}

impl Drop for DirtyLog {
    fn drop(&mut self) {
        unsafe { libc::close(self.vm_fd); }
    }
}

/* Writes KVM queued instead of exiting to us, see VM::coalesce_pio().
 * KVM appends at `last` and the reader frees entries by advancing
 * `first`; when the ring is full, a write exits as usual. The ring is
 * the VM's, so it has one reader, which must apply what is queued
 * before handling an exit for the same ports, or writes reorder. */
pub struct CoalescedRing {
    ring: *mut kvm_coalesced_mmio_ring
}

/* Only the reader touches `first`, and it is behind the device's lock. */
unsafe impl Send for CoalescedRing {}

impl CoalescedRing {
    const ENTRIES: u32 = ((PAGE_SIZE as usize - std::mem::size_of::<kvm_coalesced_mmio_ring>())
                          / std::mem::size_of::<kvm_coalesced_mmio>()) as u32;

    /* Calls write(port, data) for each queued write, oldest first. */
    pub fn drain(&self, mut write: impl FnMut(u64, &[u8])) {
        let first = unsafe { std::ptr::addr_of_mut!((*self.ring).first) };
        let last = unsafe { std::ptr::addr_of!((*self.ring).last) };
        let entries = unsafe { (*self.ring).__bindgen_anon_1.coalesced_mmio.as_ptr() };
        let mut i = read_guest_ptr::<u32>(first.cast());
        /* Entries up to `last` are complete once it is read. */
        let end = read_guest_ptr::<u32>(last.cast());
        std::sync::atomic::fence(Ordering::Acquire);
        while i != end {
            let entry = unsafe { std::ptr::read_volatile(entries.add(i as usize)) };
            write(entry.phys_addr, &entry.data[..(entry.len as usize).min(entry.data.len())]);
            i = (i + 1) % Self::ENTRIES;
        }
        /* KVM may reuse the entries once `first` passes them. */
        std::sync::atomic::fence(Ordering::Release);
        write_guest_ptr::<u32>(first.cast(), end);
    }
}

impl Drop for CoalescedRing {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ring.cast(), PAGE_SIZE as usize); }
    }
}

/* The mapping lives as long as the last Arc to the region, and the
 * guest writes it concurrently anyway; accesses go through volatile reads and writes. */
unsafe impl Send for MemRegion {}
unsafe impl Sync for MemRegion {}

impl Drop for MemRegion {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.mem_ptr, self.mem_size); }
    }
}

fn gpa_to_host(mem: &GuestMemory, gpa: u64, len: usize) -> Option<*mut u8> {
    mem.iter().find_map(|r| {
        let off = usize::try_from(gpa.checked_sub(r.guest_phys_addr)?).ok()?;
        (off.checked_add(len)? <= r.mem_size).then(|| unsafe { (r.mem_ptr as *mut u8).add(off) })
    })
}

fn read_guest_ptr<T>(ptr: *const u8) -> T {
    unsafe {
        std::ptr::read_volatile(ptr.cast::<T>())
    }
}

fn write_guest_ptr<T>(ptr: *mut u8, data: T) {
    unsafe {
        std::ptr::write_volatile(ptr.cast::<T>(), data)
    }
}

impl VM {
    fn create_vcpu(&self, id: u32, kvm_run_size: usize) -> io::Result<VCPU> {
        // 140904 ioctl(9<anon_inode:kvm-vm>, 0xae41 /* KVM_CREATE_VCPU */, 0) = 10<anon_inode:kvm-vcpu:0>
        let vcpu_fd = unsafe { libc::ioctl(self.fd, KVM_CREATE_VCPU, id as usize) };
        if vcpu_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut vcpu = VCPU { id, fd: vcpu_fd, kvm_run_mem: std::ptr::null_mut(), kvm_run_size, breakpoints: vec![] };
        vcpu.map_kvm_run()?;
        Ok(vcpu)
    }

    fn add_mem_region(&mut self, mem_size: usize, guest_phys_addr: u64) -> io::Result<usize> {
        // 140900 mmap(NULL, 1075838976, 0 /* PROT_NONE */, 0x22 /* MAP_PRIVATE|MAP_ANONYMOUS */, -1, 0) = 0x7768b3e00000
        // 140900 mmap(0x7768b3e00000, 1073741824, 0x3 /* PROT_READ|PROT_WRITE */, 0x32 /* MAP_PRIVATE|MAP_FIXED|MAP_ANONYMOUS */, -1, 0) = 0x7768b3e00000
        let mem_ptr = unsafe {
            libc::mmap(std::ptr::null_mut(),
                       mem_size as usize,
                       libc::PROT_READ|libc::PROT_WRITE,
                       libc::MAP_PRIVATE|libc::MAP_ANONYMOUS,
                       -1,
                       0)
        };
        if mem_ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        let id = self.next_slot;
        self.next_slot += 1;
        self.set_user_memory_region(id, 0, guest_phys_addr, mem_size as u64, mem_ptr as u64)?;
        self.slots.push(Slot { id, guest_phys_addr, size: mem_size as u64, userspace_addr: mem_ptr as u64, logged: false });
        self.guest_memory.push(Arc::new(MemRegion { mem_ptr, mem_size, guest_phys_addr, allocators: vec![] }));

        Ok(self.guest_memory.len() - 1)
    }

    /* A memory_size of 0 deletes the slot. */
    fn set_user_memory_region(&self, slot: u32, flags: u32, guest_phys_addr: u64,
                              memory_size: u64, userspace_addr: u64) -> io::Result<()> {
        // 140900 ioctl(9<anon_inode:kvm-vm>, 0x4020ae46 /* KVM_SET_USER_MEMORY_REGION */, {slot=0, flags=0, guest_phys_addr=0, memory_size=1073741824, userspace_addr=0x7768b3e00000}) = 0
        let region = kvm_userspace_memory_region { slot, flags, guest_phys_addr, memory_size, userspace_addr };
        let ret = unsafe { libc::ioctl(self.fd, KVM_SET_USER_MEMORY_REGION, &region) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /* Starts logging writes to the pages covering gpa..gpa + size. Logging
     * is per slot, and a slot cannot be resized, so the slot holding the
     * range is deleted and added again as up to three: before, logged,
     * after. The mapping stays one piece. The range must not overlap one
     * logged before; call before the vcpus run. */
    pub fn log_dirty_pages(&mut self, gpa: u64, size: usize) -> io::Result<DirtyLog> {
        let start = gpa & !(PAGE_SIZE - 1);
        let end = (gpa + size as u64).next_multiple_of(PAGE_SIZE);
        let idx = self.slots.iter()
            .position(|s| !s.logged && s.guest_phys_addr <= start && end <= s.guest_phys_addr + s.size)
            .ok_or(io::Error::other(format!("No unlogged memory slot covers {:#x}..{:#x}.", start, end)))?;

        /* Its own fd, as frontends may fetch it after the VM is dropped.
         * Taken first, so a failure here leaves the slots untouched. */
        let vm_fd = unsafe { libc::dup(self.fd) };
        if vm_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let old = &self.slots[idx];
        let (old_id, old_gpa, old_size, old_host) = (old.id, old.guest_phys_addr, old.size, old.userspace_addr);
        let saved_next_slot = self.next_slot;
        let mut free_id = Some(old_id);
        let pieces: Vec<Slot> = [(old_gpa, start, false), (start, end, true), (end, old_gpa + old_size, false)]
            .into_iter()
            .filter(|(from, to, _)| from != to)
            .map(|(from, to, log)| {
                let id = free_id.take().unwrap_or_else(|| {
                    self.next_slot += 1;
                    self.next_slot - 1
                });
                Slot { id, guest_phys_addr: from, size: to - from,
                       userspace_addr: old_host + (from - old_gpa), logged: log }
            })
            .collect();

        if let Err(e) = self.replace_slot(old_id, old_gpa, old_host, &pieces) {
            /* Put KVM back as it was: drop the pieces added, re-add the
             * whole slot. Best effort, the first error is the one returned. */
            for piece in &pieces {
                let _ = self.set_user_memory_region(piece.id, 0, piece.guest_phys_addr, 0, piece.userspace_addr);
            }
            let _ = self.set_user_memory_region(old_id, 0, old_gpa, old_size, old_host);
            self.next_slot = saved_next_slot;
            unsafe { libc::close(vm_fd) };
            return Err(e);
        }

        let logged = pieces.iter().find(|s| s.logged).unwrap().id;
        self.slots.remove(idx);
        self.slots.extend(pieces);
        Ok(DirtyLog { vm_fd, gpa: start, slot: logged, pages: ((end - start) / PAGE_SIZE) as usize })
    }

    /* Deletes slot old_id and adds pieces in its place. On error, KVM may
     * hold some of the pieces; the caller undoes it. */
    fn replace_slot(&self, old_id: u32, old_gpa: u64, old_host: u64, pieces: &[Slot]) -> io::Result<()> {
        self.set_user_memory_region(old_id, 0, old_gpa, 0, old_host)?;
        for piece in pieces {
            let flags = if piece.logged { KVM_MEM_LOG_DIRTY_PAGES } else { 0 };
            self.set_user_memory_region(piece.id, flags, piece.guest_phys_addr, piece.size, piece.userspace_addr)?;
        }
        Ok(())
    }

    /* Makes KVM queue the guest's writes to ports..ports + len in the
     * returned ring rather than exit for each; reads still exit. None
     * if KVM can't. Once per VM: the ring is shared by every zone. It
     * is mapped through a vcpu, and on its own, so a frontend can keep
     * reading it after the vcpus are gone. */
    pub fn coalesce_pio(&self, vcpu: &VCPU, port: u16, len: u32) -> io::Result<Option<CoalescedRing>> {
        let supported = unsafe { libc::ioctl(self.fd, KVM_CHECK_EXTENSION, KVM_CAP_COALESCED_PIO as usize) };
        if supported <= 0 {
            return Ok(None);
        }
        let zone = kvm_coalesced_mmio_zone {
            addr: port as u64,
            size: len,
            __bindgen_anon_1: kvm_coalesced_mmio_zone__bindgen_ty_1 { pio: 1 }
        };
        let ret = unsafe { libc::ioctl(self.fd, KVM_REGISTER_COALESCED_MMIO, &zone) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        let ring = unsafe {
            libc::mmap(std::ptr::null_mut(),
                       PAGE_SIZE as usize,
                       libc::PROT_READ|libc::PROT_WRITE,
                       libc::MAP_SHARED,
                       vcpu.fd,
                       (KVM_COALESCED_MMIO_PAGE_OFFSET as u64 * PAGE_SIZE) as libc::off_t)
        };
        if ring == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(CoalescedRing { ring: ring.cast() }))
    }

    fn load_data_to_memory(&self, mem_region_idx: usize, data: Vec<u8>, offset: u64) -> io::Result<usize> {
        let mem_region = self.guest_memory
            .get(mem_region_idx)
            .ok_or(io::Error::other("Index exceeds memory region vector."))?;

        if (offset as usize) >= mem_region.mem_size {
            return Err(io::Error::other("Offset exceeds memory region."));
        }

        if (offset as usize) + data.len() > mem_region.mem_size {
            return Err(io::Error::other(format!("Data ({:#x} {:#x}) would exceed memory region ({}).", offset, data.len(), mem_region.mem_size)));
        }

        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(),
                                          (mem_region.mem_ptr as *mut u8).add(offset as usize),
                                          data.len());
        };

        Ok(data.len())
    }

    fn load_file_to_memory(&self, mem_region_idx: usize, path: &str, offset: u64) -> io::Result<usize> {
        let res = read_file(path)?;

        println!("Loading {:?} at {:#x}...\r", path, offset + self.guest_memory[mem_region_idx].guest_phys_addr);

        self.load_data_to_memory(mem_region_idx, res, offset)
    }

    /* The region to add allocators to or fill through them: only while
     * nothing else holds it, i.e. while the VM is being set up. Only the
     * x86 loader uses allocators so far. */
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    fn mem_region_mut(&mut self, mem_region_idx: usize) -> io::Result<&mut MemRegion> {
        let region = self.guest_memory
            .get_mut(mem_region_idx)
            .ok_or(io::Error::other("Index exceeds memory region vector."))?;
        Arc::get_mut(region).ok_or(io::Error::other("Memory region is already shared."))
    }

    fn load_linux(&mut self, vcpu: &mut VCPU, config: &VmConfig) -> io::Result<()> {
        self.arch_load_linux(vcpu, config)
    }

    fn init_mmio_devices(&mut self, config: &VmConfig) -> io::Result<()> {
        self.arch_init_mmio_devices(config)
    }

    fn init_io_devices(&mut self, vcpu: &VCPU, config: &VmConfig) -> io::Result<()> {
        self.arch_init_io_devices(vcpu, config)
    }

    fn add_mmio_device(&mut self, addr: u64, device: Arc<Mutex<dyn MMIODevice>>) {
        self.mmio_devices.insert(addr >> 12, device);
    }

    fn handle_mmio(&self, mmio: &mut MMIO) -> io::Result<()> {
        let page = mmio.phys_addr >> 12;
        if let Some(device) = self.mmio_devices.get(&page) {
            device.lock().unwrap().handle(mmio, &self.guest_memory)?;
        }
        Ok(())
    }

    fn handle_io(&self, io: &IO, base: *mut u8) -> io::Result<()> {
        self.arch_handle_io(io, base)
    }

    fn poll_mmio_devices(&self) -> io::Result<()> {
        for device in self.mmio_devices.values() {
            device.lock().unwrap().poll(&self.guest_memory)?;
        }
        Ok(())
    }

    /* Runs each vcpu on its own thread until the VM stops. The first vcpu
     * to leave its run loop stops the others; the first error is returned. */
    fn run(self, vcpus: Vec<VCPU>) -> io::Result<()> {
        unsafe { install_kick_handler() };

        let vm = Arc::new(self);
        let mut threads = vec![];
        let mut res = Ok(());
        for mut vcpu in vcpus {
            let vm_thread = vm.clone();
            let thread = std::thread::Builder::new()
                .name(format!("vcpu{}", vcpu.id))
                .spawn(move || {
                    let res = vcpu.run(&vm_thread);
                    vm_thread.control.stop();
                    res
                });
            match thread {
                Ok(thread) => threads.push(thread),
                Err(e) => { vm.control.stop(); res = Err(e); break; }
            }
        }

        for thread in threads {
            let thread_res = thread.join()
                .unwrap_or_else(|_| Err(io::Error::other("vCPU thread panicked.")));
            if res.is_ok() {
                res = thread_res;
            }
        }
        res
    }
}

impl Drop for VM {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd); }
    }
}

pub struct VCPU {
    pub id: u32,
    pub fd: libc::c_int,
    pub kvm_run_mem: *mut libc::c_void,
    kvm_run_size: usize,
    /* Set by set_breakpoints, in debug register order on x86. */
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    breakpoints: Vec<Breakpoint>
}

/* kvm_run is only touched by the thread running the vcpu. */
unsafe impl Send for VCPU {}

impl VCPU {
    fn map_kvm_run(&mut self) -> io::Result<()> {
        // 140904 mmap(NULL, 12288, 0x3 /* PROT_READ|PROT_WRITE */, 0x1 /* MAP_SHARED */, 10<anon_inode:kvm-vcpu:0>, 0) = 0x7769050b0000
        self.kvm_run_mem = unsafe {
            libc::mmap(std::ptr::null_mut(),
                       self.kvm_run_size,
                       libc::PROT_READ|libc::PROT_WRITE,
                       libc::MAP_SHARED,
                       self.fd,
                       0)
        };
        if self.kvm_run_mem == libc::MAP_FAILED {
            self.kvm_run_mem = std::ptr::null_mut();
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /* Runs the vcpu on the calling thread until it exits or the VM stops. */
    fn run(&mut self, vm: &VM) -> io::Result<()> {
        KVM_RUN_PTR.set(self.kvm_run_mem as *mut kvm_run);
        vm.control.register_vcpu_thread();
        let res = self.run_loop(vm);
        vm.control.unregister_vcpu_thread();
        KVM_RUN_PTR.set(std::ptr::null_mut());
        res
    }

    fn run_loop(&mut self, vm: &VM) -> io::Result<()> {
        let run = self.kvm_run_mem as *mut kvm_run;

        loop {
            if vm.control.should_stop() {
                break;
            }
            if vm.control.take_devices_pending() {
                vm.poll_mmio_devices()?;
            }

            let ret = unsafe { libc::ioctl(self.fd, KVM_RUN, 0usize) };
            if ret < 0 {
                /* Taken first: print_regs() makes ioctls of its own. */
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EINTR) {
                    unsafe { std::ptr::write_volatile(&mut (*run).immediate_exit, 0); }
                    continue;
                }
                self.print_regs()?;
                return Err(io::Error::new(e.kind(), format!("KVM_RUN: {}", e)));
            }

            let exit_reason = unsafe { (*run).exit_reason };

            match exit_reason {
                KVM_EXIT_IO => {
                    let io = unsafe { (*run).__bindgen_anon_1.io };
                    vm.handle_io(&io, self.kvm_run_mem as *mut u8)?;
                }
                KVM_EXIT_SHUTDOWN => {
                    println!("Guest shutdown.");
                    self.print_regs()?;
                    break; }
                KVM_EXIT_MMIO => {
                    let mmio = unsafe { &mut (*run).__bindgen_anon_1.mmio };
                    vm.handle_mmio(mmio)?
                }
                KVM_EXIT_DEBUG => {
                    self.handle_debug_exit()?;
                }
                KVM_EXIT_HLT => {
                    println!("Guest halted.");
                    break; }
                KVM_EXIT_INTERNAL_ERROR => {
                    let internal = unsafe { (*run).__bindgen_anon_1.internal };
                    println!("suberror: {:#x}", internal.suberror);
                    for i in 0..internal.ndata {
                        println!("{:4}: {:#x}", i, internal.data[i as usize]);
                    }
                    return Err(io::Error::other("KVM internal error."));
                }
                KVM_EXIT_SYSTEM_EVENT => {
                    let event = unsafe { (*run).__bindgen_anon_1.system_event };
                    match event.type_ {
                        KVM_SYSTEM_EVENT_SHUTDOWN => {
                            println!("Guest powered off.\r");
                            break;
                        }
                        /* There is no reset yet: stop as on a power off. */
                        KVM_SYSTEM_EVENT_RESET => {
                            println!("Guest asked for a reset; stopping.\r");
                            break;
                        }
                        KVM_SYSTEM_EVENT_CRASH => return Err(io::Error::other("Guest crashed (KVM_SYSTEM_EVENT_CRASH).")),
                        other => return Err(io::Error::other(format!("Unexpected KVM system event {}.", other)))
                    }
                }
                KVM_EXIT_FAIL_ENTRY => {
                    let fail_entry = unsafe { (*run).__bindgen_anon_1.fail_entry };
                    let reason = fail_entry.hardware_entry_failure_reason;
                    let cpu = fail_entry.cpu;
                    self.print_regs()?;
                    return Err(io::Error::other(format!(
                        "KVM could not enter the guest: hardware entry failure reason {:#x} on CPU {}.", reason, cpu)));
                }
                _ => {
                    return Err(io::Error::other(format!("Unexpected KVM exit reason {}.", exit_reason)));
                }
            }
        }

        Ok(())
    }
}

impl Drop for VCPU {
    fn drop(&mut self) {
        if !self.kvm_run_mem.is_null() {
            unsafe { libc::munmap(self.kvm_run_mem, self.kvm_run_size); }
        }
        unsafe { libc::close(self.fd); }
    }
}

/* immediate_exit makes KVM_RUN return EINTR even if the kick lands before the vcpu enters the guest. */
extern "C" fn kick_handler(_sig: libc::c_int) {
    let run = KVM_RUN_PTR.get();
    if !run.is_null() {
        unsafe { std::ptr::write_volatile(&mut (*run).immediate_exit, 1); }
    }
}

/* Real-time signal, so it can't clash with SIGUSR1/2 users. */
fn kick_signal() -> libc::c_int {
    libc::SIGRTMIN()
}

/* Must run before any vcpu thread registers: the default action would kill us. */
unsafe fn install_kick_handler() {
    let mut sa: libc::sigaction = std::mem::zeroed();
    sa.sa_sigaction = kick_handler as *const() as usize;
    libc::sigemptyset(&mut sa.sa_mask);
    sa.sa_flags = 0;
    libc::sigaction(kick_signal(), &sa, std::ptr::null_mut());
}

/* Names the calling thread, and with it every thread it starts unnamed;
 * the main thread's name is the process's. The kernel keeps 15 bytes. */
fn set_process_name(name: &str) -> io::Result<()> {
    let mut comm = [0u8; 16];
    let len = name.floor_char_boundary(15);
    comm[..len].copy_from_slice(&name.as_bytes()[..len]);
    if unsafe { libc::prctl(libc::PR_SET_NAME, comm.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn print_dirty_pages(log: &DirtyLog) -> io::Result<()> {
    let bitmap = log.fetch()?;
    let pages: Vec<u64> = (0..bitmap.len() * 64)
        .filter(|n| bitmap[n / 64] & (1 << (n % 64)) != 0)
        .map(|n| log.gpa + n as u64 * PAGE_SIZE)
        .collect();
    if pages.is_empty() {
        println!("Dirty log: no page written.\r");
    }
    for page in pages {
        println!("Dirty log: {:#x} written.\r", page);
    }
    Ok(())
}

fn main() {
    let args = Args::parse();
    /* The serial frontend's thread may never get to restore the terminal
     * when the guest stops the VM, or when setting up fails after it put
     * the terminal in raw mode, so main restores it. */
    let saved_tty = save_tty();
    let res = args.vm_config().and_then(|config| run(&args, &config));
    if let Some(tty) = saved_tty {
        restore_tty(&tty);
    }
    if let Err(e) = res {
        eprintln!("hearth: {}", e);
        std::process::exit(1);
    }
}

fn run(args: &Args, config: &VmConfig) -> io::Result<()> {
    if let Some(name) = &config.name {
        set_process_name(name)?;
    }
    let kvm_dev = KvmDev::new()?;
    let mut vm = kvm_dev.create_vm()?;

    arch::arch_pre_vcpu_init(&kvm_dev, &mut vm)?;

    let mut vcpu = vm.create_vcpu(0, kvm_dev.get_kvm_run_size())?;

    match &config.name {
        Some(name) => println!("\nHearth: {}\r", name),
        None => println!("\nHearth\r")
    }

    arch::arch_init(&kvm_dev, &mut vm, &mut vcpu)?;

    vcpu.set_ip(config.load_addr as usize)?;

    /* Guest memory is complete before any device (and device thread) exists. */
    if let Some(_) = config.linux {
        vm.load_linux(&mut vcpu, config)?;
    } else if let Some(binary) = &config.binary {
        let mem_region_idx = vm.add_mem_region(config.memory.unwrap_or(256 << 10), 0x0)?;
        vm.load_file_to_memory(mem_region_idx, binary, config.load_addr)?;
    }

    if !args.breakpoints.is_empty() {
        let elf_data = match &config.linux {
            Some(path) => Some(read_file(path)?),
            None => None
        };
        let breakpoints = args.breakpoints.iter()
            .map(|spec| resolve_breakpoint(spec, elf_data.as_deref()))
            .collect::<io::Result<Vec<_>>>()?;
        vcpu.set_breakpoints(breakpoints)?;
        for bp in &vcpu.breakpoints {
            println!("Breakpoint {} at {:#x}.\r", bp.label, bp.addr);
        }
    }

    vm.init_mmio_devices(config)?;
    vm.init_io_devices(&vcpu, config)?;
    let dirty_log = match args.dirty_log {
        Some((gpa, size)) => Some(vm.log_dirty_pages(gpa, size)?),
        None => None
    };
    if let Some(addr) = &config.vnc {
        start_rfb(addr.as_str(), vm.display.clone(), vm.keyboard.clone())?;
    }

    let res = vm.run(vec![vcpu]);
    if let Some(log) = dirty_log {
        /* The VM's own error matters more; only report this one. */
        if let Err(e) = print_dirty_pages(&log) {
            eprintln!("Dirty log: cannot fetch: {}\r", e);
        }
    }
    res
}
