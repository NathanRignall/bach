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

pub fn typescript() -> String {
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

    let mut out = String::from(
        "// Generated from crates/bach-protocol by `cargo test -p bach-protocol`. Don't edit.\n\n",
    );
    for decl in c.decls.values() {
        out += decl;
        out += "\n";
    }
    out + &commands::typescript_map(&cfg)
}
