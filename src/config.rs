/* The VM's configuration: from a TOML file (--config), then from the
 * command line, whose flags override the file's settings. Each flag
 * --foo X is foo = X in the file; --disk's KEY=VALUE settings are the
 * keys of its [disk] table. Relative paths are taken from the current
 * directory, as on the command line, not from the file's. */

use std::fs;
use std::io;
use serde::{ Deserialize, Deserializer };
use serde::de::Error;
use crate::{ parse_number, parse_serial, SerialSpec };

pub const DEFAULT_LOAD_ADDR: u64 = 0x1000;
pub const DEFAULT_BLK_QUEUE_SIZE: u32 = 128;
const MAX_QUEUE_SIZE: u32 = 32768;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VmConfig {
    pub name: Option<String>,
    pub binary: Option<String>,
    #[serde(alias = "kernel")]
    pub linux: Option<String>,
    /* In bytes; written as for --memory, e.g. "1G", or as a number of bytes. */
    #[serde(deserialize_with = "de_mem_size")]
    pub memory: Option<usize>,
    pub load_addr: u64,
    pub dtb: Option<String>,
    pub initramfs: Option<String>,
    pub append: Option<String>,
    pub disk: Option<DiskConfig>,
    pub tap: Option<String>,
    pub vnc: Option<String>,
    /* Written as for --serial, e.g. "pty" or "unix:/tmp/vm.sock". */
    #[serde(deserialize_with = "de_serial")]
    pub serial: SerialSpec
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            name: None,
            binary: None,
            linux: None,
            memory: None,
            load_addr: DEFAULT_LOAD_ADDR,
            dtb: None,
            initramfs: None,
            append: None,
            disk: None,
            tap: None,
            vnc: None,
            serial: SerialSpec::Stdio
        }
    }
}

impl VmConfig {
    pub fn load(path: &str) -> io::Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|e| io::Error::new(e.kind(), format!("Failed to read {}: {}", path, e)))?;
        Self::from_toml(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {}", path, e)))
    }

    fn from_toml(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if let Some(disk) = &config.disk {
            disk.validate().map_err(|e| format!("[disk]: {}", e))?;
        }
        Ok(config)
    }

    /* Checks what only the merged configuration can tell. */
    pub fn validate(&self) -> Result<(), String> {
        match (&self.binary, &self.linux) {
            (Some(_), Some(_)) => return Err("give either binary or linux, not both".into()),
            (None, None) => return Err("nothing to run: give --linux or --binary \
                                        (or linux or binary in the --config file)".into()),
            _ => ()
        }
        if let Some(memory) = self.memory {
            /* KVM maps guest memory in whole host pages. */
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
            if memory % page_size != 0 {
                return Err(format!("memory must be a multiple of the page size ({}), got {}",
                                   page_size, memory));
            }
        }
        Ok(())
    }
}

/* A size in bytes, with an optional K, M or G suffix. */
pub fn parse_mem_size(arg: &str) -> Result<usize, String> {
    let (num, shift) = match arg.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&arg[..i], match c.to_ascii_uppercase() {
            'K' => 10,
            'M' => 20,
            'G' => 30,
            _ => return Err(format!("unknown size suffix '{}'", c))
        }),
        _ => (arg, 0)
    };
    let num: usize = num.parse().map_err(|e| format!("invalid size {}: {}", arg, e))?;
    num.checked_mul(1 << shift)
        .filter(|size| *size > 0)
        .ok_or(format!("invalid size {}", arg))
}

/* memory = "512M", or memory = 536870912 in bytes, as on the command line. */
fn de_mem_size<'de, D: Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    struct SizeVisitor;
    impl<'de> serde::de::Visitor<'de> for SizeVisitor {
        type Value = usize;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a size in bytes, or a string with a K, M or G suffix, e.g. \"512M\"")
        }
        fn visit_u64<E: Error>(self, n: u64) -> Result<usize, E> {
            usize::try_from(n).ok().filter(|n| *n > 0).ok_or(E::custom(format!("invalid size {}", n)))
        }
        fn visit_i64<E: Error>(self, n: i64) -> Result<usize, E> {
            u64::try_from(n).map_err(|_| E::custom(format!("invalid size {}", n)))
                .and_then(|n| self.visit_u64(n))
        }
        fn visit_str<E: Error>(self, s: &str) -> Result<usize, E> {
            parse_mem_size(s).map_err(E::custom)
        }
    }
    d.deserialize_any(SizeVisitor).map(Some)
}

fn de_serial<'de, D: Deserializer<'de>>(d: D) -> Result<SerialSpec, D::Error> {
    parse_serial(&String::deserialize(d)?).map_err(D::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskConfig {
    pub path: String,
    /* Entries in the request queue, offered as QueueNumMax. */
    #[serde(default = "default_blk_queue_size")]
    pub queue_size: u32,
    /* Data segments per request, offered with VIRTIO_BLK_F_SEG_MAX. */
    pub seg_max: Option<u32>
}

fn default_blk_queue_size() -> u32 { DEFAULT_BLK_QUEUE_SIZE }

impl DiskConfig {
    fn validate(&self) -> Result<(), String> {
        if self.path.is_empty() {
            return Err("no disk path given".into());
        }
        check_range("queue_size", self.queue_size, 1, MAX_QUEUE_SIZE)?;
        if let Some(seg_max) = self.seg_max {
            check_range("seg_max", seg_max, 1, u32::MAX)?;
        }
        Ok(())
    }
}

fn check_range(key: &str, n: u32, min: u32, max: u32) -> Result<(), String> {
    if (min..=max).contains(&n) {
        Ok(())
    } else {
        Err(format!("{} must be in {}..={}, got {}", key, min, max, n))
    }
}

/* --disk PATH[,queue_size=N][,seg_max=N], or path=PATH anywhere in the
 * list. A path with a comma can't be given; one with '=' needs path=. */
pub fn parse_disk(arg: &str) -> Result<DiskConfig, String> {
    let mut path = None;
    let mut queue_size = None;
    let mut seg_max = None;

    for (i, item) in arg.split(',').enumerate() {
        let (key, value) = match item.split_once('=') {
            Some(kv) => kv,
            None if i == 0 && !item.is_empty() => ("path", item),
            None => return Err(format!("expected KEY=VALUE, got '{}'", item))
        };
        let seen = match key {
            "path" => path.replace(value.to_string()).is_some(),
            "queue_size" => queue_size.replace(parse_u32(key, value)?).is_some(),
            "seg_max" => seg_max.replace(parse_u32(key, value)?).is_some(),
            _ => return Err(format!("unknown key '{}' (known: path, queue_size, seg_max)", key))
        };
        if seen {
            return Err(format!("{} given more than once", key));
        }
    }

    let disk = DiskConfig {
        path: path.unwrap_or_default(),
        queue_size: queue_size.unwrap_or(DEFAULT_BLK_QUEUE_SIZE),
        seg_max
    };
    disk.validate()?;
    Ok(disk)
}

fn parse_u32(key: &str, value: &str) -> Result<u32, String> {
    let n = parse_number(value).map_err(|e| format!("{}: {}", key, e))?;
    u32::try_from(n).map_err(|_| format!("{}: {} is too large", key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(path: &str, queue_size: u32, seg_max: Option<u32>) -> DiskConfig {
        DiskConfig { path: path.to_string(), queue_size, seg_max }
    }

    #[test]
    fn bare_path() {
        assert_eq!(parse_disk("rootfs.img"), Ok(disk("rootfs.img", DEFAULT_BLK_QUEUE_SIZE, None)));
    }

    #[test]
    fn options() {
        assert_eq!(parse_disk("rootfs.img,queue_size=256,seg_max=126"),
                   Ok(disk("rootfs.img", 256, Some(126))));
        assert_eq!(parse_disk("seg_max=0x10,path=/a=b.img"),
                   Ok(disk("/a=b.img", DEFAULT_BLK_QUEUE_SIZE, Some(16))));
    }

    #[test]
    fn errors() {
        for bad in ["", "queue_size=4", "path=", "a.img,", "a.img,b.img", "a.img,path=b.img",
                    "a.img,queue_size=0", "a.img,queue_size=32769", "a.img,seg_max=0",
                    "a.img,seg_max=x",
                    "a.img,seg_max=1,seg_max=2", "a.img,ro=on"] {
            assert!(parse_disk(bad).is_err(), "accepted {:?}", bad);
        }
    }
}
