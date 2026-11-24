//! `oneios-tool image tweaks`: OneiOS's root tweaks and the image build's userland hook, in Rust (the port of
//! userland/roottweak and userland/image-hook; tools/oneios-tool/README.md "image tweaks").
//!
//! The property lists are read and written by the `plist` crate (plist.rs), the JSON by serde_json (oneios-common's
//! json); the report's words are tweaks.py's. No program is started and no file of Apple's is needed in git: the tweaks
//! read the extracted volumes they are given, and the three config files are built in (config.rs).
//!
//! Entry points: `hook::run_hook` (what build.sh's hook does; `serve`'s `tweaks` request, the image build on the
//! phone), `apply::apply` (`roottweak apply`), `bootcheck::check`, `macho::describe`, and `run` for the command line.
//! A long loop asks `job::check()` (a stop of the request) and counts the bytes it writes (`job::count`).

pub mod apply;
pub mod bootcheck;
pub mod cli;
pub mod config;
pub mod hook;
pub mod launchd;
pub mod macho;
pub mod plist;
pub mod shlex;
pub mod tree;
pub mod tweaks;

pub use apply::{apply, Applied, Apply};
pub use hook::{preflight_metal, run_hook, Hook, HookDone};
pub use tweaks::Options;

use oneios_common::job;
use oneios_common::json::Value;
use std::path::Path;

/// roottweak's SCHEMA
pub const SCHEMA: i64 = 1;

/// A wrong input (roottweak's TreeError and ConfigError): the tree, the config, an option. `serve` answers it as
/// refused, `input`; the command line prints it and exits 1.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Refused {}

/// An entry of owner-trust.json's "files".
pub(crate) fn trust_entry(path: &str, s: &macho::Slice, warnings: &[String]) -> Value {
    let mut e = Value::dict();
    e.set("path", path);
    e.set("cdhash", s.cdhash.as_str());
    e.set("hash", s.hash);
    e.set("identifier", s.identifier.as_str());
    e.set("arch", s.arch.as_str());
    e.set("page_size", s.page_size);
    e.set("adhoc", s.adhoc);
    e.set("warnings", Value::List(warnings.iter().map(Value::str).collect()));
    e
}

/// Write a file of OUT as NAME.part, then rename it over NAME (a stop or an error leaves no part).
pub(crate) fn write_file(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    job::check()?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    let part = job::temp(path.with_file_name(name));
    std::fs::write(part.path(), data).map_err(|e| anyhow::anyhow!("{}: {e}", part.path().display()))?;
    std::fs::rename(part.path(), path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    part.keep();
    job::count(data.len() as u64);
    Ok(())
}

/// `oneios-tool image tweaks ARGS...`; the exit status.
pub fn run(args: &[String]) -> i32 {
    cli::run(args)
}
