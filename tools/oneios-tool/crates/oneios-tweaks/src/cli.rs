//! `oneios-tool image tweaks apply|daemons|cdhash|boot-check|hook` (roottweak/cli.py's commands, and image-hook).

use crate::apply::Apply;
use crate::tweaks::{Options, PREF_GROUPS};
use crate::{bootcheck, config, hook, macho, Refused};
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "oneios-tool image tweaks", args_override_self = true, about = "The root tweaks and the image build's userland hook (the port of userland/roottweak and userland/image-hook)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug, Clone, Default)]
pub struct Policy {
    /// another userland/config directory (default: the one built into the tool)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// also disable this daemon group
    #[arg(long, value_name = "NAME")]
    pub group: Vec<String>,
    /// do not disable this group (or "replace")
    #[arg(long = "no-group", value_name = "NAME")]
    pub no_group: Vec<String>,
    /// keep this job although a group names it
    #[arg(long, value_name = "LABEL")]
    pub keep: Vec<String>,
}

#[derive(Args, Debug, Clone)]
pub struct ApplyArgs {
    /// the extracted OS volume (/)
    #[arg(long, required_unless_present = "root_image", conflicts_with = "root_image")]
    pub root: Option<PathBuf>,
    /// or the OS volume's image (raw APFS: os.img), read in place without a mount (sealed only)
    #[arg(long = "root-image", value_name = "IMAGE")]
    pub root_image: Option<PathBuf>,
    /// the volume of --root-image: a number or a name (default: the first)
    #[arg(long = "root-volume", value_name = "N|NAME", requires = "root_image")]
    pub root_volume: Option<String>,
    /// the tree that becomes the Data volume (/private/var); with --layout rw default ROOT/private/var
    #[arg(long)]
    pub data: Option<PathBuf>,
    /// where the manifest, the boot contract and the report go
    #[arg(long)]
    pub out: PathBuf,
    /// sealed: the OS volume is Apple's image, nothing is written to it (default). rw: a root we write
    #[arg(long, default_value = "sealed", value_parser = ["sealed", "rw"])]
    pub layout: String,
    #[command(flatten)]
    pub policy: Policy,
    /// disable this job, even one the policy keeps
    #[arg(long, value_name = "LABEL")]
    pub disable: Vec<String>,
    /// the UI scale: 3 (default, 480x1040 points) or 2 (720x1560 points), at the panel's full resolution either way
    #[arg(long, value_parser = clap::value_parser!(i64).range(2..=3))]
    pub scale: Option<i64>,
    /// the build number, where SystemVersion.plist has none
    #[arg(long)]
    pub build: Option<String>,
    /// apply to a root that is not the profile's iOS version
    #[arg(long = "any-version")]
    pub any_version: bool,
    /// signed guest binaries: the output of userland/helpers/build.sh
    #[arg(long, value_name = "DIR")]
    pub helpers: Option<PathBuf>,
    /// the capture plug-in: the output of userland/camera/build.sh (H9ISP.mediacapture)
    #[arg(long = "camera-plugin", value_name = "DIR")]
    pub camera_plugin: Option<PathBuf>,
    /// a built, signed Metal plugin bundle (NativeAdrenoMetal.bundle)
    #[arg(long = "metal-plugin", value_name = "BUNDLE")]
    pub metal_plugin: Option<String>,
    /// place a plugin bundle that does not say it was built for this guest (no oneios-target file)
    #[arg(long = "metal-plugin-unchecked")]
    pub metal_plugin_unchecked: bool,
    /// a process that gets the Metal device ("*": all)
    #[arg(long = "metal-client", value_name = "NAME")]
    pub metal_client: Vec<String>,
    /// the extracted Cryptex1,AppOS volume
    #[arg(long, value_name = "DIR")]
    pub appos: Option<PathBuf>,
    /// leave the "setup" preferences out
    #[arg(long = "no-setup")]
    pub no_setup: bool,
    /// leave the "migration" preferences out
    #[arg(long = "no-migration")]
    pub no_migration: bool,
    /// leave the "watchdogs" preferences out
    #[arg(long = "no-watchdogs")]
    pub no_watchdogs: bool,
    /// leave the "awake" preferences out
    #[arg(long = "no-awake")]
    pub no_awake: bool,
    /// leave the "coreanimation" preferences out
    #[arg(long = "no-coreanimation")]
    pub no_coreanimation: bool,
    /// leave the "dictation" preference out (the keyboard then loops on dictation availability with no network)
    #[arg(long = "no-dictation")]
    pub no_dictation: bool,
    /// Reduce Motion and Reduce Transparency on
    #[arg(long = "lighter-visuals")]
    pub lighter_visuals: bool,
    /// write ios-sweet's plain systembag.kb (only for a kernel that reads one)
    #[arg(long = "plain-keybag")]
    pub plain_keybag: bool,
    /// rw: leave RecoverOnCrash in the jetsam properties
    #[arg(long = "keep-recover-on-crash")]
    pub keep_recover_on_crash: bool,
}

impl ApplyArgs {
    pub fn to_apply(&self) -> Apply {
        let flags = [self.no_setup, self.no_migration, self.no_watchdogs, self.no_awake, self.no_coreanimation, self.no_dictation];
        Apply {
            root: self.root.clone().unwrap_or_default(),
            root_image: self.root_image.clone(),
            root_volume: self.root_volume.clone(),
            data: self.data.clone(),
            out: self.out.clone(),
            rw: self.layout == "rw",
            config: self.policy.config.clone(),
            scale: self.scale,
            options: Options {
                build: self.build.clone(),
                any_version: self.any_version,
                groups: self.policy.group.clone(),
                without_groups: self.policy.no_group.clone(),
                keep: self.policy.keep.clone(),
                disable: self.disable.clone(),
                keep_recover_on_crash: self.keep_recover_on_crash,
                helpers: self.helpers.clone(),
                metal_plugin: self.metal_plugin.clone(),
                camera_plugin: self.camera_plugin.clone(),
                metal_plugin_unchecked: self.metal_plugin_unchecked,
                metal_clients: self.metal_client.clone(),
                appos: self.appos.clone(),
                no_prefs: PREF_GROUPS.iter().zip(flags).filter(|(_, on)| *on).map(|(g, _)| g.to_string()).collect(),
                lighter_visuals: self.lighter_visuals,
                plain_keybag: self.plain_keybag,
            },
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "apply", args_override_self = true, no_binary_name = true)]
struct ApplyOnly {
    #[command(flatten)]
    args: ApplyArgs,
}

/// `apply`'s options from words (the hook's: image-hook's argument list and ONEIOS_ROOTTWEAK_ARGS).
pub fn parse_apply(words: &[String]) -> Result<Apply> {
    match ApplyOnly::try_parse_from(words) {
        Ok(a) => Ok(a.args.to_apply()),
        Err(e) => Err(Refused(format!("apply: {}", e.to_string().trim_end())).into()),
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// apply the tweaks to an extracted root and its Data tree (roottweak apply)
    Apply(ApplyArgs),
    /// print the daemon policy
    Daemons(Policy),
    /// CDHash of signed Mach-O files
    Cdhash {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// after a boot: did launchd apply the daemon policy (reads the Data volume)
    BootCheck {
        /// the files of the Data volume a boot left (/private/var)
        #[arg(long, value_name = "DIR")]
        data: PathBuf,
        /// the files of the OS volume: adds the jobs that run at load and the DisableAllowed marks
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
        /// the tree was copied by a user who is not root: do not check the override's owner
        #[arg(long = "ignore-owner")]
        ignore_owner: bool,
    },
    /// the image build's userland hook, API 1: ONEIOS_* as scripts/image/build.sh sets them (userland/image-hook)
    Hook {
        /// the checkout: its out/userland/{helpers,NativeAdrenoMetal.bundle,camera} are the defaults, as
        /// image-hook's, and its userland/config is read
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// another userland/config directory
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// check that an S24 image has a target-matched Metal bundle or runtime NGPUADD2 addon before copying images
    MetalPreflight {
        device: String,
        build: String,
        #[arg(long = "metal-plugin", value_name = "BUNDLE")]
        metal_plugin: Option<PathBuf>,
        #[arg(long = "runtime-addon", value_name = "FILE")]
        runtime_addon: Option<PathBuf>,
        #[arg(long = "allow-no-metal")]
        allow_no_metal: bool,
    },
}

fn fail(e: anyhow::Error) -> i32 {
    if let Some(r) = e.downcast_ref::<Refused>() {
        eprintln!("oneios-tool image tweaks: {r}");
    } else {
        eprintln!("oneios-tool image tweaks: {e:#}");
    }
    1
}

fn daemons(p: &Policy) -> Result<()> {
    let d = config::daemons(p.config.as_deref())?;
    let chosen = d.selected(&p.group, &p.no_group, &p.keep)?;
    let chosen: std::collections::HashSet<&str> = chosen.iter().map(|(l, _)| l.as_str()).collect();
    let mut keep: Vec<&String> = d.keep.iter().collect();
    keep.sort();
    println!("keep ({}): never disabled", d.keep.len());
    for label in keep {
        match d.keep_reasons.get(label) {
            Some(why) => println!("  {label}  # {why}"),
            None => println!("  {label}"),
        }
    }
    let mut idle = d.idle.clone();
    idle.sort();
    println!("\nidle ({}): kept running without their hardware", idle.len());
    for (label, why) in &idle {
        println!("  {label}  # {why}");
    }
    let mut replace = d.replace.clone();
    replace.sort();
    println!("\nreplace ({}): disabled, done by the host", replace.len());
    for (label, why) in &replace {
        println!("  {label}  # {why}");
    }
    let mut groups: Vec<&(String, config::Group)> = d.groups.iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, g) in groups {
        let on = g.labels.iter().all(|l| chosen.contains(l.as_str()));
        println!("\ngroup {name} ({}, {}): {}", g.labels.len(), if on { "on" } else { "off" }, g.source);
        for label in &g.labels {
            println!("  {label}");
        }
    }
    println!("\n{} jobs disabled with this selection", chosen.len());
    Ok(())
}

fn cdhash(files: &[PathBuf]) -> i32 {
    let mut status = 0;
    for file in files {
        let shown = file.display();
        match std::fs::read(file) {
            Err(e) => {
                eprintln!("{shown}: {}", bootcheck::os_error(&e, file));
                status = 1;
            }
            Ok(data) => match macho::describe(&data) {
                Ok(slices) => {
                    for s in slices {
                        println!(
                            "{} {} pages={} adhoc={} platform={} minos={} id={} {shown}",
                            s.cdhash,
                            s.arch,
                            s.page_size,
                            s.adhoc as u8,
                            s.platform.as_deref().unwrap_or("None"),
                            s.minos.as_deref().unwrap_or("None"),
                            s.identifier
                        );
                    }
                }
                Err(e) => {
                    eprintln!("{shown}: {e}");
                    status = 1;
                }
            },
        }
    }
    status
}

pub fn run(args: &[String]) -> i32 {
    let cli = match Cli::try_parse_from(std::iter::once("oneios-tool image tweaks".to_string()).chain(args.iter().cloned())) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    match cli.command {
        Command::Apply(a) => match crate::apply::apply(&a.to_apply()) {
            Ok(_) => 0,
            Err(e) => fail(e),
        },
        Command::Daemons(p) => match daemons(&p) {
            Ok(()) => 0,
            Err(e) => fail(e),
        },
        Command::Cdhash { files } => cdhash(&files),
        Command::BootCheck { data, root, ignore_owner } => {
            let (lines, problems) = bootcheck::check(&data, root.as_deref(), ignore_owner);
            for l in lines {
                println!("{l}");
            }
            for p in &problems {
                eprintln!("boot-check: {p}");
            }
            if problems.is_empty() { 0 } else { 1 }
        }
        Command::Hook { repo, config } => {
            let env = |k: &str| std::env::var(k).ok();
            let run = || -> Result<()> {
                let mut h = hook::Hook::from_env(&env, repo.as_deref())?;
                h.config = config.clone().or_else(|| repo.as_ref().map(|r| r.join("userland/config")));
                hook::run_hook(&h)?;
                Ok(())
            };
            match run() {
                Ok(()) => 0,
                Err(e) => fail(e),
            }
        }
        Command::MetalPreflight { device, build, metal_plugin, runtime_addon, allow_no_metal } => {
            match hook::preflight_metal_with_addon(
                &device,
                &build,
                metal_plugin.as_deref(),
                runtime_addon.as_deref(),
                allow_no_metal,
            ) {
                Ok(()) => 0,
                Err(e) => fail(e),
            }
        }
    }
}
