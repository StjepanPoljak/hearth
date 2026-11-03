use std::io::{ self, Write };
use termion::raw::IntoRawMode;
use std::io::{ stdin, stdout, Read };
use std::fs::{ File, OpenOptions };
use std::os::fd::{ AsRawFd, FromRawFd, RawFd };
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixListener;
use crate::VmControl;
use std::sync::{ Arc, Mutex };
use std::collections::VecDeque;

pub trait TTY: Send {
    fn send_char(&mut self, ch: u8) -> io::Result<()>;
}

/* Where the guest's serial port is connected, from --serial. */
#[derive(Clone, Debug)]
pub enum SerialSpec {
    /* This terminal, in raw mode; Ctrl-A x stops the VM. */
    Stdio,
    /* A new pseudo-terminal, for screen or minicom. */
    Pty,
    /* Output only, appended to a file. */
    File(String),
    /* A Unix socket, one client at a time. */
    Unix(String)
}

pub fn parse_serial(arg: &str) -> Result<SerialSpec, String> {
    match (arg, arg.split_once(':')) {
        ("stdio", _) => Ok(SerialSpec::Stdio),
        ("pty", _) => Ok(SerialSpec::Pty),
        (_, Some(("file", path))) if !path.is_empty() => Ok(SerialSpec::File(path.into())),
        (_, Some(("unix", path))) if !path.is_empty() => Ok(SerialSpec::Unix(path.into())),
        _ => Err("expected stdio, pty, file:PATH or unix:PATH".into())
    }
}

/* The guest's output, shared by the UART and the thread that connects
 * clients. A client that stops reading must not stall the vcpu, so pty
 * and socket writes don't block: what doesn't fit is dropped, as on a
 * line with nobody listening. A socket without a client keeps the last
 * BACKLOG bytes for the next one, so attaching late still shows the boot. */
#[derive(Clone, Default)]
pub struct SerialOut(Arc<Mutex<SerialSink>>);

#[derive(Default)]
struct SerialSink {
    out: Option<Box<dyn Write + Send>>,
    backlog: Option<VecDeque<u8>>
}

const BACKLOG: usize = 64 << 10;

impl SerialOut {
    fn keep_backlog(&self) {
        self.0.lock().unwrap().backlog = Some(VecDeque::new());
    }

    fn connect(&self, out: Box<dyn Write + Send>) {
        let mut sink = self.0.lock().unwrap();
        sink.out = Some(out);
        if let Some(backlog) = sink.backlog.as_mut() {
            let kept: Vec<u8> = backlog.drain(..).collect();
            sink.write(&kept);
        }
    }

    fn disconnect(&self) {
        self.0.lock().unwrap().out = None;
    }

    pub fn write(&self, bytes: &[u8]) {
        self.0.lock().unwrap().write(bytes);
    }
}

impl SerialSink {
    fn write(&mut self, bytes: &[u8]) {
        if let Some(w) = self.out.as_mut() {
            match w.write_all(bytes).and_then(|_| w.flush()) {
                Err(e) if e.kind() != io::ErrorKind::WouldBlock => self.out = None,
                _ => return
            }
        }
        if let Some(backlog) = self.backlog.as_mut() {
            backlog.extend(bytes);
            let excess = backlog.len().saturating_sub(BACKLOG);
            backlog.drain(..excess);
        }
    }
}

pub fn save_tty() -> Option<libc::termios> {
    let mut tty: libc::termios = unsafe { std::mem::zeroed() };
    (unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut tty) } == 0).then_some(tty)
}

pub fn restore_tty(tty: &libc::termios) {
    unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, tty); }
}

/* Connects the serial port as `spec` says: returns where the UART writes
 * the guest's output, and starts a thread feeding input to `uart`. */
pub fn start_serial(spec: &SerialSpec, uart: Arc<Mutex<dyn TTY>>, control: Arc<VmControl>) -> io::Result<SerialOut> {
    let out = SerialOut::default();
    match spec {
        SerialSpec::Stdio => {
            out.connect(Box::new(stdout()));
            start_tty(uart, control);
        }
        SerialSpec::Pty => {
            let (master, path) = open_pty()?;
            println!("Serial console on {}\r", path);
            out.connect(Box::new(master.try_clone()?));
            std::thread::spawn(move || feed(master, &uart));
        }
        SerialSpec::File(path) => {
            let file = OpenOptions::new().create(true).append(true).open(path)
                .map_err(|e| io::Error::new(e.kind(), format!("--serial file:{}: {}", path, e)))?;
            out.connect(Box::new(file));
        }
        SerialSpec::Unix(path) => {
            /* Replace a socket left behind by an earlier run, nothing else. */
            if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket()) {
                std::fs::remove_file(path)?;
            }
            let listener = UnixListener::bind(path)
                .map_err(|e| io::Error::new(e.kind(), format!("--serial unix:{}: {}", path, e)))?;
            println!("Serial console on unix:{}\r", path);
            out.keep_backlog();
            let out = out.clone();
            std::thread::spawn(move || {
                for client in listener.incoming() {
                    let Ok(client) = client else { continue };
                    let (Ok(()), Ok(writer)) = (client.set_nonblocking(true), client.try_clone()) else { continue };
                    out.connect(Box::new(writer));
                    feed(client, &uart);
                    out.disconnect();
                }
            });
        }
    }
    Ok(out)
}

/* Passes what `input` reads to the guest until it hits end of file. The
 * fd may be non-blocking, so it waits with poll(). */
fn feed(mut input: impl Read + AsRawFd, uart: &Arc<Mutex<dyn TTY>>) {
    let mut buf = [0u8; 256];
    loop {
        let mut pfd = libc::pollfd { fd: input.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, -1) } < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        match input.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                let mut uart = uart.lock().unwrap();
                for &b in &buf[..n] {
                    uart.send_char(b).ok();
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => (),
            Err(_) => return
        }
    }
}

/* A pseudo-terminal in raw mode; returns its master and the slave's path.
 * The slave stays open (its fd is never closed), so reads on the master
 * wait for a client instead of failing while nobody has it open. */
fn open_pty() -> io::Result<(File, String)> {
    let check = |ret: libc::c_int| if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret) };
    let master = check(unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK) })?;
    let master = unsafe { File::from_raw_fd(master) };
    check(unsafe { libc::grantpt(master.as_raw_fd()) })?;
    check(unsafe { libc::unlockpt(master.as_raw_fd()) })?;

    let mut name = [0 as libc::c_char; 64];
    check(unsafe { libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()) })?;
    let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned();

    let slave: RawFd = check(unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) })?;
    let mut tty: libc::termios = unsafe { std::mem::zeroed() };
    unsafe {
        libc::tcgetattr(slave, &mut tty);
        libc::cfmakeraw(&mut tty);
        libc::tcsetattr(slave, libc::TCSANOW, &tty);
    }
    Ok((master, path))
}

/* The terminal, raw if it is one; Ctrl-A x stops the VM. */
fn start_tty(uart_async: Arc<Mutex<dyn TTY>>, control: Arc<VmControl>) {
    std::thread::spawn(move || {
        let mut is_escape = false;
        let mut quit = false;
        /* Input may be a pipe or /dev/null: then there is no raw mode to set. */
        let raw = stdout().into_raw_mode().ok();
        const QUIT_BYTE : u8 = 'x' as u8;

        for byte in stdin().bytes() {
            let Ok(b) = byte else { break };

            if !is_escape && b == 0x01 {
                is_escape = true;
                continue;
            } else if is_escape {
                is_escape = false;
                match b {
                    QUIT_BYTE => { quit = true; break; },
                    0x01 => (),
                    _ => { continue; }
                };
            }

            /* The byte is queued even if raising the IRQ fails, as in feed():
             * the guest still finds it, and this thread must not panic. */
            uart_async.lock().unwrap().send_char(b).ok();
        }

        /* A pipe running dry leaves the guest running; a terminal going
         * away doesn't. */
        if quit || raw.is_some() {
            drop(raw);
            control.stop();
        }
    });
}
