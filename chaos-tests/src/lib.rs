#![allow(
    unused,
    dead_code,
    non_upper_case_globals,
    non_camel_case_types,
    unused_assignments,
    unused_mut
)]

pub(crate) use std::any::Any;
pub(crate) use std::cell::RefCell;
pub(crate) use std::cmp::{max, min, Ordering as CmpOrd};
pub(crate) use std::collections::{BTreeMap, BTreeSet, HashMap, LinkedList, VecDeque};
pub(crate) use std::fmt;
pub(crate) use std::ops::{Deref, DerefMut, Index};
pub(crate) use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};
pub(crate) use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
pub(crate) use std::thread;
pub(crate) use std::time::Duration;

pub mod consts;
pub mod fs;
pub mod ipc;
pub mod memory;
pub mod process;
pub mod sync;
pub mod trap;
pub mod util;

pub use consts::*;
pub use fs::*;
pub use ipc::*;
pub use memory::*;
pub use process::abi::*;
pub use process::futex::*;
pub use process::proc::*;
pub use process::schedule::*;
pub use process::structs::*;
pub use process::thread::*;
pub use sync::*;
pub use trap::*;
pub use util::*;

// AGENT: Compatibility logging kept at the crate root while kernel.rs is split.
pub(crate) fn chaos_log_enabled(module: &str) -> bool {
    let raw = match std::env::var("CHAOS_LOG") {
        Ok(v) => v,
        Err(_) => "gkl".to_string(),
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("false") {
        return false;
    }
    if raw == "1" || raw.eq_ignore_ascii_case("true") || raw.eq_ignore_ascii_case("all") {
        return true;
    }
    raw.split(',').any(|part| {
        let part = part.trim();
        part.eq_ignore_ascii_case(module) || (module == "gkl" && part.eq_ignore_ascii_case("locks"))
    })
}

// AGENT: Central log printer; use CHAOS_LOG=gkl,cache,kernel,fs,mm,task or CHAOS_LOG=all.
pub(crate) fn chaos_log<F>(module: &str, msg: F)
where
    F: FnOnce() -> String,
{
    if chaos_log_enabled(module) {
        println!(
            "[chaos:{} tid={:?}] {}",
            module,
            thread::current().id(),
            msg()
        );
    }
}
