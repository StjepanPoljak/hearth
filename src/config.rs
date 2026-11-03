/* Device configuration, given on the command line as PATH[,KEY=VALUE...]. */

use crate::parse_number;

pub const DEFAULT_BLK_QUEUE_SIZE: u32 = 128;
const MAX_QUEUE_SIZE: u32 = 32768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskConfig {
    pub path: String,
    /* Entries in the request queue, offered as QueueNumMax. */
    pub queue_size: u32,
    /* Data segments per request, offered with VIRTIO_BLK_F_SEG_MAX. */
    pub seg_max: Option<u32>
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
            "queue_size" => queue_size.replace(parse_u32(key, value, 1, MAX_QUEUE_SIZE)?).is_some(),
            "seg_max" => seg_max.replace(parse_u32(key, value, 1, u32::MAX)?).is_some(),
            _ => return Err(format!("unknown key '{}' (known: path, queue_size, seg_max)", key))
        };
        if seen {
            return Err(format!("{} given more than once", key));
        }
    }

    let path = path.filter(|p| !p.is_empty()).ok_or("no disk path given")?;
    Ok(DiskConfig {
        path,
        queue_size: queue_size.unwrap_or(DEFAULT_BLK_QUEUE_SIZE),
        seg_max
    })
}

fn parse_u32(key: &str, value: &str, min: u32, max: u32) -> Result<u32, String> {
    let n = parse_number(value).map_err(|e| format!("{}: {}", key, e))?;
    u32::try_from(n).ok()
        .filter(|n| (min..=max).contains(n))
        .ok_or(format!("{} must be in {}..={}, got {}", key, min, max, value))
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
                    "a.img,seg_max=x", "a.img,seg_max=1,seg_max=2", "a.img,ro=on"] {
            assert!(parse_disk(bad).is_err(), "accepted {:?}", bad);
        }
    }
}
