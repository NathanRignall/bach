//! Other Unixes: nothing to look at, so no process is ever found. bach-tasks can start tasks but
//! can't follow them.
use super::Stat;
use std::path::PathBuf;

pub(crate) fn stat(_pid: u32) -> Option<Stat> {
    None
}

pub(crate) fn all_pids() -> Vec<u32> {
    vec![]
}

pub(crate) fn argv(_pid: u32) -> Vec<String> {
    vec![]
}

pub(crate) fn name(_pid: u32) -> Option<String> {
    None
}

pub(crate) fn cwd(_pid: u32) -> Option<PathBuf> {
    None
}

pub(crate) fn age_secs(_pid: u32) -> Option<u64> {
    None
}

pub(crate) fn socket_owners() -> Vec<(u16, Option<u32>)> {
    vec![]
}
