//! The image build's userland hook, hook API 1 (userland/image-hook; docs/image-format.md section 6): `apply` in
//! the sealed layout on the copied Data tree, then the first-start unpacker into the overlay and the trust list,
//! the Metal plug-in's build record, the owner rules and the summary.
//!
//! `run_hook` is the library's entry point (`serve`'s `tweaks` request, the image build on the phone);
//! `oneios-tool image tweaks hook` reads the same fields from ONEIOS_* as image-hook does (`from_env`).

use crate::apply::{self, Applied};
use crate::cli;
use crate::macho;
use crate::tree::{is_dir, is_file};
use crate::{write_file, Refused};
use anyhow::{bail, Context as _, Result};
use oneios_common::json::{self, Value};
use oneios_common::pyrepr::str_value;
use std::path::{Path, PathBuf};

/// What build.sh gives the hook (the ONEIOS_* of hook API 1), as fields.
#[derive(Clone, Debug, Default)]
pub struct Hook {
    /// ONEIOS_OS_ROOT: the OS volume's files (read only)
    pub os_root: PathBuf,
    /// or the OS volume's image (raw APFS, `os.img`), read in place with our APFS reader: on the phone, which
    /// mounts nothing. When given, `os_root` is not read.
    pub os_image: Option<PathBuf>,
    /// ONEIOS_DATA: the copied Data tree (tree-copy's), changed in place
    pub data: PathBuf,
    /// ONEIOS_OVERLAY: the image set's overlay/; the files go to overlay/userland/
    pub overlay: PathBuf,
    /// ONEIOS_BUILD: the build, where SystemVersion.plist has none
    pub build: Option<String>,
    /// ONEIOS_DEVICE: image target, used to reject a mismatched Metal bundle
    pub device: Option<String>,
    /// signed guest helpers (userland/helpers/build.sh's output): used when it is a folder
    pub helpers: Option<PathBuf>,
    /// the Metal plug-in bundle: placed when it is a folder; its target and build record are checked against the image
    pub metal_plugin: Option<PathBuf>,
    /// explicit escape hatch for an intentionally CPU-only S24 Ultra image (ONEIOS_ALLOW_NO_METAL=1)
    pub allow_no_metal: bool,
    /// the capture plug-in's folder: used when it holds H9ISP.mediacapture
    pub camera_plugin: Option<PathBuf>,
    /// ONEIOS_SCALE: the UI scale, "2" or "3" (as the environment gives it)
    pub scale: Option<String>,
    /// another userland/config folder; None: the one built into the tool
    pub config: Option<PathBuf>,
    /// more `apply` options (ONEIOS_ROOTTWEAK_ARGS, split like a shell line): --group fewest, --disable LABEL ...
    pub args: Vec<String>,
    /// ONEIOS_OWNERS: the owner rules are appended here (path, uid, gid, mode; tab-separated)
    pub owners: Option<PathBuf>,
    /// ONEIOS_INFO: the summary (JSON) is written here
    pub info: Option<PathBuf>,
}

/// What the hook did.
#[derive(Clone, Debug)]
pub struct HookDone {
    pub build: String,
    /// report.txt's text
    pub report: String,
    /// the folder of the outputs (OVERLAY/userland)
    pub out: PathBuf,
    pub disabled_jobs: usize,
    pub owner_cdhashes: usize,
    /// overlay/userland/bin/oneios-seed when the helpers had it
    pub first_start_unpacker: Option<String>,
    /// "item: note" of each item that is off
    pub off: Vec<String>,
    pub not_found: Vec<String>,
    /// the summary written to `info`
    pub info: Value,
}

impl Hook {
    /// The fields from ONEIOS_* as image-hook reads them; `repo` gives its defaults (REPO/out/userland/helpers,
    /// .../NativeAdrenoMetal.bundle, .../camera), where the variables are not set.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>, repo: Option<&Path>) -> Result<Hook> {
        let api = env("ONEIOS_HOOK_API");
        if api.as_deref() != Some("1") {
            let shown = api.as_deref().map_or_else(|| "None".to_string(), oneios_common::pyrepr::repr_str);
            bail!(Refused(format!("image-hook: hook API {shown}, this hook speaks 1")));
        }
        let need = |k: &str| env(k).map(PathBuf::from).ok_or_else(|| Refused(format!("image-hook: {k} is not set")));
        let (data, overlay) = (need("ONEIOS_DATA")?, need("ONEIOS_OVERLAY")?);
        // (ONEIOS_OS_IMAGE: this tool's own addition, the image instead of a mount)
        let os_image = env("ONEIOS_OS_IMAGE").filter(|s| !s.is_empty()).map(PathBuf::from);
        let os_root = if os_image.is_some() { env("ONEIOS_OS_ROOT").map(PathBuf::from).unwrap_or_default() } else { need("ONEIOS_OS_ROOT")? };
        let given = |k: &str, default: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| repo.map(|r| r.join(default)));
        let args = match env("ONEIOS_ROOTTWEAK_ARGS") {
            Some(text) => crate::shlex::split(&text).map_err(|e| Refused(format!("ONEIOS_ROOTTWEAK_ARGS: {e}")))?,
            None => Vec::new(),
        };
        Ok(Hook {
            os_root,
            os_image,
            data,
            overlay,
            build: env("ONEIOS_BUILD").filter(|b| !b.is_empty()),
            device: env("ONEIOS_DEVICE").filter(|d| !d.is_empty()),
            helpers: given("ONEIOS_HELPERS", "out/userland/helpers"),
            metal_plugin: given("ONEIOS_METAL_PLUGIN", "out/userland/NativeAdrenoMetal.bundle"),
            allow_no_metal: env("ONEIOS_ALLOW_NO_METAL").is_some_and(|value| value == "1"),
            camera_plugin: given("ONEIOS_CAMERA_PLUGIN", "out/userland/camera"),
            scale: env("ONEIOS_SCALE").filter(|s| !s.is_empty()),
            config: None,
            args,
            owners: env("ONEIOS_OWNERS").filter(|s| !s.is_empty()).map(PathBuf::from),
            info: env("ONEIOS_INFO").filter(|s| !s.is_empty()).map(PathBuf::from),
        })
    }
}

fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn metal_plugin_to_place(h: &Hook) -> Option<&Path> {
    if h.device.as_deref() == Some("iPhone18,2") && h.allow_no_metal {
        return None;
    }
    h.metal_plugin.as_deref().filter(|p| is_dir(p))
}

/// Validate the target-specific Metal input before an image build does expensive work.
pub fn preflight_metal(device: &str, build: &str, plugin: Option<&Path>, allow_no_metal: bool) -> Result<()> {
    let h = Hook {
        device: Some(device.to_string()),
        build: Some(build.to_string()),
        metal_plugin: plugin.map(Path::to_path_buf),
        allow_no_metal,
        ..Hook::default()
    };
    check_metal_target(&h)
}

fn validate_metal_target(expected: &str, bundle: &str, record: Option<&str>) -> Result<()> {
    if bundle.trim() != expected {
        bail!(Refused(format!(
            "image-hook: Metal bundle targets {:?}, but this image is {expected}; rebuild it with ONEIOS_METAL_TARGET='{expected}'",
            bundle.trim()
        )));
    }
    if let Some(record) = record {
        if record != expected {
            bail!(Refused(format!(
                "image-hook: Metal build record targets {record}, but this image is {expected}; rebuild the bundle for this profile"
            )));
        }
    }
    Ok(())
}

/// Refuse an S24 Ultra image that would silently fall back to CPU composition, and refuse a plug-in built for another
/// image when the profile identity is available. The explicit CPU-only escape is for deliberate fallback builds.
fn check_metal_target(h: &Hook) -> Result<()> {
    let required = h.device.as_deref() == Some("iPhone18,2") && !h.allow_no_metal;
    let Some(plugin) = metal_plugin_to_place(h) else {
        if required {
            bail!(Refused(
                "image-hook: the S24 Ultra image requires a matching NativeAdrenoMetal.bundle; build it with scripts/bg.sh metal userland/metal/build.sh (set ONEIOS_ALLOW_NO_METAL=1 only for an intentional CPU-only image)".into()
            ));
        }
        return Ok(());
    };
    let (Some(device), Some(build)) = (h.device.as_deref(), h.build.as_deref()) else {
        if required {
            bail!(Refused("image-hook: cannot verify the S24 Ultra Metal target without ONEIOS_DEVICE and ONEIOS_BUILD".into()));
        }
        return Ok(());
    };
    let expected = format!("{device} {build}");
    let target_path = plugin.join("oneios-target");
    if !is_file(&target_path) {
        if required {
            bail!(Refused(format!(
                "image-hook: {} has no oneios-target file; rebuild the Metal bundle for {expected}",
                plugin.display()
            )));
        }
        return Ok(());
    }
    let bundle_target = std::fs::read_to_string(&target_path).with_context(|| target_path.display().to_string())?;
    let record_path = plugin.with_extension("build.json");
    let record_target = if is_file(&record_path) {
        json::load(&record_path)?.get("target").and_then(Value::as_str).map(str::to_owned)
    } else {
        None
    };
    if required && record_target.is_none() {
        bail!(Refused(format!(
            "image-hook: {} is missing its Metal build record; rebuild it with userland/metal/build.sh",
            record_path.display()
        )));
    }
    validate_metal_target(&expected, &bundle_target, record_target.as_deref())
}

/// The hook: `apply --layout sealed` and the rest of image-hook, with the same files as it leaves.
pub fn run_hook(h: &Hook) -> Result<HookDone> {
    check_metal_target(h)?;
    let out = h.overlay.join("userland");
    let mut argv: Vec<String> = vec!["--layout".into(), "sealed".into()];
    match &h.os_image {
        Some(image) => argv.extend(["--root-image".into(), path_text(image)]),
        None => argv.extend(["--root".into(), path_text(&h.os_root)]),
    }
    argv.extend(["--data".into(), path_text(&h.data), "--out".into(), path_text(&out)]);
    if let Some(b) = &h.build {
        argv.extend(["--build".into(), b.clone()]);
    }
    if let Some(helpers) = h.helpers.as_ref().filter(|p| is_dir(p)) {
        argv.extend(["--helpers".into(), path_text(helpers)]);
    }
    if let Some(plugin) = metal_plugin_to_place(h) {
        argv.extend(["--metal-plugin".into(), path_text(plugin)]);
    }
    if let Some(camera) = h.camera_plugin.as_ref().filter(|p| is_file(&p.join("H9ISP.mediacapture"))) {
        argv.extend(["--camera-plugin".into(), path_text(camera)]);
    }
    if let Some(s) = &h.scale {
        argv.extend(["--scale".into(), s.clone()]);
    }
    if let Some(c) = &h.config {
        argv.extend(["--config".into(), path_text(c)]);
    }
    argv.extend(h.args.iter().cloned());
    let a = cli::parse_apply(&argv)?;
    let Applied { build, report, .. } = apply::apply(&a)?;

    // The first-start unpacker runs before the Data volume exists, so it cannot be in the seed: it goes into the
    // overlay for xnuhost to hand to the guest, and its CDHash into the owner trust list.
    let trust_path = out.join("owner-trust.json");
    let mut first_start = None;
    if let Some(seed_tool) = h.helpers.as_ref().map(|d| d.join("oneios-seed")).filter(|p| is_file(p)) {
        let binary = std::fs::read(&seed_tool).with_context(|| seed_tool.display().to_string())?;
        let slices = macho::describe(&binary).map_err(|e| Refused(format!("image-hook: {}: {e}; the guest runs only signed code", seed_tool.display())))?;
        std::fs::create_dir_all(out.join("bin"))?;
        let dest = out.join("bin").join("oneios-seed");
        write_file(&dest, &binary)?;
        std::fs::set_permissions(&dest, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        let mut trust = json::load(&trust_path)?;
        let mut files: Vec<Value> = trust.get("files").and_then(Value::as_list).map(<[Value]>::to_vec).unwrap_or_default();
        for s in &slices {
            files.push(crate::trust_entry("overlay/userland/bin/oneios-seed", s, &[]));
        }
        let mut cdhashes: Vec<String> = files.iter().filter_map(|t| t.get("cdhash").map(str_value)).collect();
        cdhashes.sort();
        cdhashes.dedup();
        trust.set("files", Value::List(files));
        trust.set("cdhashes", Value::List(cdhashes.into_iter().map(Value::Str).collect()));
        write_file(&trust_path, (json::dumps(&trust, Some(2), true) + "\n").as_bytes())?;
        first_start = Some("overlay/userland/bin/oneios-seed".to_string());
    }

    // The plug-in's build record (NativeAdrenoMetal.build.json beside the bundle, with the CDHash of each binary as the
    // build signed it): when the bundle was placed, the trust list must hold exactly those. A bundle changed after
    // its build stops the image build here, not at the guest's AMFI.
    let mut metal = Value::Null;
    if let Some(plugin) = &h.metal_plugin {
        let text = path_text(plugin);
        let text = text.trim_end_matches('/');
        if let Some(stem) = text.strip_suffix(".bundle") {
            let record = PathBuf::from(format!("{stem}.build.json"));
            if is_file(&record) {
                let built = json::load(&record)?;
                let trust = json::load(&trust_path)?;
                let contract = json::load(out.join("boot-contract.json"))?;
                let placed = contract.get("metal").is_some();
                let built_hashes = built.get("cdhashes").cloned().ok_or_else(|| Refused(format!("{}: no \"cdhashes\"", record.display())))?;
                if placed {
                    let have: std::collections::BTreeSet<String> = trust.get("cdhashes").and_then(Value::as_list).unwrap_or(&[]).iter().map(str_value).collect();
                    let mut missing: Vec<String> = built_hashes.as_list().unwrap_or(&[]).iter().map(str_value).filter(|c| !have.contains(c)).collect();
                    missing.sort();
                    missing.dedup();
                    if !missing.is_empty() {
                        bail!(Refused(format!(
                            "image-hook: {} lists CDHashes the placed bundle does not have ({}): the bundle changed after it was built; run userland/metal/build.sh again",
                            record.display(),
                            missing.join(", ")
                        )));
                    }
                }
                let sdk = match built.get("sdk") {
                    Some(s) if oneios_common::pyrepr::truthy(Some(s)) => s.get("version").cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                let mut m = Value::dict();
                m.set("placed", placed);
                m.set("target", built.get("target").cloned().unwrap_or(Value::Null));
                m.set("cdhashes", built_hashes);
                m.set("sdk", sdk);
                m.set("record", record.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                metal = m;
            }
        }
    }

    let manifest = json::load(out.join("tweaks.manifest.json"))?;
    if let Some(owners) = &h.owners {
        let mut text = std::fs::read(owners).unwrap_or_default();
        for e in manifest.get("entries").and_then(Value::as_list).unwrap_or(&[]) {
            if e.get("volume").and_then(Value::as_str) == Some("data") && e.get("uid").is_some() {
                let f = |k: &str| e.get(k).map(str_value).unwrap_or_default();
                let mode = e.get("mode").map(str_value).unwrap_or_else(|| "-".into());
                text.extend_from_slice(format!("{}\t{}\t{}\t{mode}\n", f("path"), f("uid"), f("gid")).as_bytes());
            }
        }
        write_file(owners, &text)?;
    }
    let report_json = json::load(out.join("report.json"))?;
    let contract = json::load(out.join("boot-contract.json"))?;
    let trust = json::load(&trust_path)?;
    let results = report_json.get("results").and_then(Value::as_list).unwrap_or(&[]).to_vec();
    let field = |r: &Value, k: &str| r.get(k).map(str_value).unwrap_or_default();
    let off: Vec<String> = results.iter().filter(|r| field(r, "state") == "off").map(|r| format!("{}: {}", field(r, "item"), field(r, "note"))).collect();
    let not_found: Vec<String> = results.iter().filter(|r| field(r, "state") == "not-found").map(|r| field(r, "item")).collect();
    let disabled_jobs = contract.get("launch_policy").and_then(|l| l.get("disable_jobs")).and_then(Value::as_list).map_or(0, <[Value]>::len);
    let owner_cdhashes = trust.get("cdhashes").and_then(Value::as_list).map_or(0, <[Value]>::len);
    let mut info = Value::dict();
    info.set("tool", "oneios-tool image tweaks");
    info.set("schema", manifest.get("schema").cloned().unwrap_or(Value::Null));
    info.set("layout", "sealed");
    info.set("build", report_json.get("build").cloned().unwrap_or(Value::Null));
    info.set("contract", "overlay/userland/boot-contract.json");
    info.set("owner_trust", "overlay/userland/owner-trust.json");
    info.set("first_start_unpacker", first_start.clone());
    info.set("disabled_jobs", disabled_jobs);
    info.set("owner_cdhashes", owner_cdhashes);
    info.set("metal_plugin", metal);
    info.set("off", Value::List(off.iter().map(Value::str).collect()));
    info.set("not_found", Value::List(not_found.iter().map(Value::str).collect()));
    if let Some(path) = &h.info {
        write_file(path, json::dumps(&info, Some(2), false).as_bytes())?;
    }
    Ok(HookDone { build, report, out, disabled_jobs, owner_cdhashes, first_start_unpacker: first_start, off, not_found, info })
}
