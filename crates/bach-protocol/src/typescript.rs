//! Generates `src/api/generated/protocol.ts`: every type reachable from the command table and the
//! frames, in one file, plus the `Commands` map.
use crate::{commands, ClientFrame, ServerFrame};
use std::{
    any::TypeId,
    collections::{BTreeMap, HashSet},
};
use ts_rs::{Config, TypeVisitor, TS};

struct Collect<'a> {
    cfg: &'a Config,
    seen: HashSet<TypeId>,
    /// By TypeScript name: some Rust types share one (`serde_json::Value` and its aliases).
    decls: BTreeMap<String, String>,
}

impl TypeVisitor for Collect<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        // Named types get a declaration; wrappers (Vec, Option, HashMap, ...) only lead to them.
        if T::output_path().is_some() {
            let docs = T::docs()
                .map(|d| format!("{}\n", d.trim_end()))
                .unwrap_or_default();
            self.decls
                .entry(T::ident(self.cfg))
                .or_insert_with(|| format!("{docs}export {}\n", T::decl(self.cfg)));
        }
        T::visit_dependencies(self);
        T::visit_generics(self);
    }
}

/// Identifies the protocol: a hash of everything in the TypeScript bindings, so it changes
/// whenever a command, event or type on the wire does. FNV-1a, which (unlike std's hasher) is the
/// same on every build.
pub fn fingerprint() -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in declarations().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn typescript() -> String {
    format!(
        "// Generated from crates/bach-protocol by `cargo test -p bach-protocol`. Don't edit.\n\n\
         /** Must match the server's `hello`; see `fingerprint` in bach-protocol. */\n\
         export const PROTOCOL = \"{}\";\n\n{}",
        fingerprint(),
        declarations()
    )
}

fn declarations() -> String {
    // Every integer on the wire (timestamps, byte offsets, counts) fits a JS number.
    let cfg = Config::new().with_large_int("number");
    let mut c = Collect {
        cfg: &cfg,
        seen: HashSet::new(),
        decls: BTreeMap::new(),
    };
    commands::visit_types(&mut c);
    c.visit::<ClientFrame>();
    c.visit::<ServerFrame>();
    c.visit::<crate::app::ConnectionStatus>();

    let mut out = String::new();
    for decl in c.decls.values() {
        out += decl;
        out += "\n";
    }
    out + &commands::typescript_map(&cfg)
}
