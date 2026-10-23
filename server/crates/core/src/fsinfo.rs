//! Which file system a folder is on (Linux `statfs`). The database keeps its read connections off on a
//! network file system (several connections to one file are slow or unsafe with their locking), and
//! Diagnostics shows the type of /config and /music so the owner can report it.
use std::path::Path;

/// The file system a folder is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsInfo {
    /// The file system as the owner knows it ("ext4", "NFS", "SMB", "FUSE", …; "unknown" for a magic
    /// number not in the list).
    pub name: &'static str,
    /// Served over the network, or through a user-space driver (FUSE: sshfs, rclone, mergerfs …) that
    /// behaves like one.
    pub network: bool,
}

/// `f_type` of statfs(2) → what it is (the numbers are linux/magic.h; a 32-bit kernel reports some of
/// them sign-extended, so only the low 32 bits count).
pub fn classify(magic: u64) -> FsInfo {
    let (name, network) = match magic & 0xFFFF_FFFF {
        0xEF53 => ("ext4", false),
        0x9123_683E => ("btrfs", false),
        0x5846_5342 => ("xfs", false),
        0xF2F5_2010 => ("f2fs", false),
        0x0102_1994 => ("tmpfs", false),
        0x794C_7630 => ("overlay", false),
        0x2FC1_2FC1 => ("zfs", false),
        0x5346_544E => ("ntfs", false),
        0x2011_BAB0 => ("exfat", false),
        0x4D44 => ("vfat", false),
        0xCA45_1A4E => ("bcachefs", false),
        0x3153_464A => ("jfs", false),
        0x5265_4973 => ("reiserfs", false),
        0x7371_7368 => ("squashfs", false),
        0x6969 => ("NFS", true),
        0x517B => ("SMB", true),
        0xFF53_4D42 => ("CIFS", true),
        0xFE53_4D42 => ("SMB2", true),
        0x6573_5546 => ("FUSE", true),
        0x0102_1997 => ("9p", true),
        0x5346_414F | 0x6B41_4653 => ("AFS", true),
        0x00C3_6400 => ("Ceph", true),
        0x7461_636F => ("OCFS2", true),
        0x0116_1970 => ("GFS2", true),
        0x7375_7245 => ("Coda", true),
        _ => ("unknown", false),
    };
    FsInfo { name, network }
}

/// The file system `path` is on; None when it cannot be asked (not Linux, or the path is missing).
#[allow(clippy::unnecessary_cast)]
pub fn of(path: &Path) -> Option<FsInfo> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut buf) } != 0 {
            return None;
        }
        Some(classify(buf.f_type as u64))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_numbers_tell_local_from_network() {
        assert_eq!(classify(0xEF53), FsInfo { name: "ext4", network: false });
        assert_eq!(classify(0x9123_683E).name, "btrfs");
        assert_eq!(classify(0x6969), FsInfo { name: "NFS", network: true });
        assert_eq!(classify(0xFF53_4D42), FsInfo { name: "CIFS", network: true });
        assert!(classify(0x517B).network);
        assert_eq!(classify(0x6573_5546), FsInfo { name: "FUSE", network: true });
        // a 32-bit kernel hands the CIFS number over sign-extended
        assert_eq!(classify(0xFFFF_FFFF_FF53_4D42).name, "CIFS");
        // something not in the list is taken as a local disk, so the read pool stays on
        assert_eq!(classify(0x1234_5678), FsInfo { name: "unknown", network: false });
    }

    #[test]
    fn asks_the_kernel_about_a_real_folder() {
        let here = of(std::path::Path::new("/"));
        if cfg!(target_os = "linux") {
            assert!(here.is_some());
        }
        assert!(of(std::path::Path::new("/no/such/folder/encore-fsinfo-test")).is_none());
    }
}
