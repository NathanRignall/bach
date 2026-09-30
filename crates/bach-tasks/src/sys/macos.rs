//! macOS: libproc for processes, `getsid` for sessions, and the `listeners` crate (libproc socket
//! info) for who listens where. Only processes we may inspect show up as socket owners: another
//! user's listening sockets aren't seen at all, where Linux reports them without an owner.
use super::Stat;
use std::{ffi::c_void, mem::MaybeUninit, path::PathBuf};

fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the buffer is a proc_bsdinfo of the size we pass.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            size,
        )
    };
    // SAFETY: the kernel filled all of it (it returns the size written).
    (n == size).then(|| unsafe { info.assume_init() })
}

pub(crate) fn stat(pid: u32) -> Option<Stat> {
    let info = bsd_info(pid)?;
    // SAFETY: plain getsid(2).
    let sid = unsafe { libc::getsid(pid as i32) };
    Some(Stat {
        zombie: info.pbi_status == libc::SZOMB,
        ppid: info.pbi_ppid,
        sid: u32::try_from(sid).ok()?,
        start: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
    })
}

pub(crate) fn all_pids() -> Vec<u32> {
    // SAFETY: a null buffer asks for the number of processes.
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return vec![];
    }
    // Room for processes started since.
    let mut pids = vec![0i32; n as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as i32;
    // SAFETY: the buffer holds `bytes` bytes.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
    pids.truncate(n.max(0) as usize);
    pids.into_iter()
        .filter_map(|p| u32::try_from(p).ok().filter(|p| *p > 0))
        .collect()
}

pub(crate) fn argv(pid: u32) -> Vec<String> {
    // KERN_PROCARGS2: argc (i32), the executable path, NUL padding, then argc NUL-terminated
    // arguments (the environment follows).
    let mut max: i32 = 0;
    let mut size = std::mem::size_of::<i32>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: reads one int into `max`.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut max as *mut i32).cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok || max <= 0 {
        return vec![];
    }
    let mut buf = vec![0u8; max as usize];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    // SAFETY: the buffer holds `size` bytes; the kernel writes at most that and updates `size`.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok || size < 4 {
        return vec![];
    }
    let buf = &buf[..size];
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let mut rest = &buf[4..];
    // Skip the executable path and the padding after it.
    let path_end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    rest = &rest[path_end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    rest.split(|b| *b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .filter(|a| !a.is_empty())
        .collect()
}

pub(crate) fn name(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer holds `buf.len()` bytes.
    let n = unsafe { libc::proc_name(pid as i32, buf.as_mut_ptr().cast::<c_void>(), 256) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

pub(crate) fn cwd(pid: u32) -> Option<PathBuf> {
    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    // SAFETY: the buffer is a proc_vnodepathinfo of the size we pass.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: filled by the kernel; the path is MAXPATHLEN bytes, NUL-terminated.
    let info = unsafe { info.assume_init() };
    let path = &info.pvi_cdir.vip_path;
    // SAFETY: [[c_char; 32]; 32] is 1024 contiguous bytes.
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), std::mem::size_of_val(path))
    };
    let len = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    (len > 0).then(|| PathBuf::from(String::from_utf8_lossy(&bytes[..len]).into_owned()))
}

pub(crate) fn age_secs(pid: u32) -> Option<u64> {
    let started = bsd_info(pid)?.pbi_start_tvsec;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(now.saturating_sub(started))
}

pub(crate) fn socket_owners() -> Vec<(u16, Option<u32>)> {
    let Ok(all) = listeners::get_all() else {
        return vec![];
    };
    all.into_iter()
        .filter(|l| {
            l.protocol == listeners::Protocol::TCP && l.state == listeners::SocketState::Listen
        })
        .map(|l| (l.socket.port(), Some(l.process.pid)))
        .collect()
}
