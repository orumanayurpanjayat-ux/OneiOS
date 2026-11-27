// SPDX-License-Identifier: MIT
//! The boot addon a runtime package carries: `kernel/gpuaddon`, the `NGPUADD2` sidecar `oneios-tool ios addon` writes
//! (the Metal plug-in under `addons/`, the capture plug-in under `camera/`), and the CDHashes `kernel/gpuaddon.cdhashes`
//! lists. The kernel writes its files into the Data store and mounts them over the sealed system's folders before
//! launchd starts (hosted 0031, 0056, 0090).
//!
//! The image set's own sidecar (`<kernel>.gpuaddon`, staged by `oneios-tool phone addon`) is the one a phone chose, and
//! it wins (session/board.rs). The runtime's is the fallback, so a plug-in reaches the guest with the runtime alone — no
//! adb, no image rebuild — and its CDHashes join the trust cache this boot gives the kernel: an image built without the
//! plug-in has no owner module that names it (`overlay/userland/owner-trust.json`).

use xh_core::bytes::le;
use xh_core::{loge, logi};

/// A boot addon and the CDHashes its code needs in this boot's trust cache.
pub struct Addon {
    pub blob: Vec<u8>,
    /// The 20-byte CDHashes of the files the addon carries (the first word of each line of the list `ios addon` writes).
    pub cdhashes: Vec<[u8; 20]>,
}

/// The runtime's boot addon: `kernel/gpuaddon` under the runtime's folder, with `kernel/gpuaddon.cdhashes`. None when
/// the runtime has none, or when the file there is not a boot addon (left alone, said in the log).
pub fn runtime_addon(runtime: &str) -> Option<Addon> {
    let path = format!("{}/kernel/gpuaddon", runtime.trim_end_matches('/'));
    let blob = crate::session::read_file(&path, 64 << 20)?;
    // The loader checks the same magic and total: "NGPUADD2" and a total that is the file's length (ios addon writes
    // 24 + the body). A cut or padded file is refused here, where the log can name the runtime.
    if blob.len() < 24 || blob.get(..8) != Some(b"NGPUADD2".as_slice()) || le::u64(&blob, 16) != blob.len() as u64 {
        loge!("gpuaddon: {path}: not a boot addon (NGPUADD2, its total is not the file's length): left alone");
        return None;
    }
    // Without its list the plug-in would land in the Data store without being trusted. Refuse the runtime fallback
    // instead; a staged image sidecar still has its own image-set trust path.
    let cdhashes = match crate::session::read_file(&format!("{path}.cdhashes"), 1 << 16) {
        Some(list) => match parse_cdhashes(&list) {
            Ok(cdhashes) if !cdhashes.is_empty() => cdhashes,
            Ok(_) => {
                loge!("gpuaddon: {path}.cdhashes lists no CDHashes: runtime add-on refused");
                return None;
            }
            Err(why) => {
                loge!("gpuaddon: {path}.cdhashes: {why}: runtime add-on refused");
                return None;
            }
        },
        None => {
            loge!("gpuaddon: {path}.cdhashes is missing: runtime add-on refused");
            return None;
        }
    };
    logi!(
        "gpuaddon: {path}: {} bytes, {} CDHashes trusted (the runtime's; the image set has no kernel.kc.gpuaddon)",
        blob.len(),
        cdhashes.len()
    );
    Some(Addon { blob, cdhashes })
}

/// Whether a GPU add-on contains the publishing marker and iPhone X 20F66 NativeAdrenoMetal bundle. Generic 20F66
/// images keep `oneios-gpu-metal=0` for the A56; xnuhost may enable it on an S24 only when this exact plug-in is
/// available to the guest. This reads just the small target and marker records, never decompresses the executable.
pub fn native_adreno_metal_20f66(blob: &[u8]) -> Result<(), String> {
    let cut = || "the gpuaddon is cut short".to_owned();
    if blob.len() < 24 || blob.get(..8) != Some(b"NGPUADD2".as_slice()) {
        return Err("the gpuaddon is not NGPUADD2".into());
    }
    if le::try_u64(blob, 16) != Some(blob.len() as u64) {
        return Err("the gpuaddon total does not match its length".into());
    }
    let count = le::try_u32(blob, 8).ok_or_else(cut)? as usize;
    if count > (blob.len() - 24) / 24 {
        return Err(cut());
    }

    const EXECUTABLE: &[u8] = b"addons/NativeAdrenoMetal.bundle/NativeAdrenoMetal";
    const TARGET: &[u8] = b"addons/NativeAdrenoMetal.bundle/oneios-target";
    const PUBLISH: &[u8] = b"addons/NativeAdrenoMetal.publish";
    let mut at = 24usize;
    let mut executable = false;
    let mut target: Option<String> = None;
    let mut publish = false;
    for _ in 0..count {
        let nlen = le::try_u32(blob, at).ok_or_else(cut)? as usize;
        let size = le::try_u64(blob, at + 8).ok_or_else(cut)?;
        let packed_len = usize::try_from(le::try_u64(blob, at + 16).ok_or_else(cut)?).map_err(|_| cut())?;
        at = at.checked_add(24).ok_or_else(cut)?;
        if nlen == 0 {
            return Err(cut());
        }
        let name_end = at.checked_add(nlen).ok_or_else(cut)?;
        let name_record = blob.get(at..name_end).ok_or_else(cut)?;
        if name_record.last() != Some(&0) {
            return Err("the gpuaddon has a path without its terminating NUL".into());
        }
        let name = &name_record[..nlen - 1];
        let padded_name_len = nlen.checked_add(7).ok_or_else(cut)? & !7;
        at = at.checked_add(padded_name_len).ok_or_else(cut)?;
        let packed_end = at.checked_add(packed_len).ok_or_else(cut)?;
        let packed = blob.get(at..packed_end).ok_or_else(cut)?;
        let padded_packed_len = packed_len.checked_add(7).ok_or_else(cut)? & !7;
        at = at.checked_add(padded_packed_len).ok_or_else(cut)?;
        if at > blob.len() {
            return Err(cut());
        }

        if name == EXECUTABLE {
            executable = size != 0;
        } else if name == TARGET {
            if target.is_some() {
                return Err("the gpuaddon contains duplicate NativeAdrenoMetal target records".into());
            }
            let bytes = xh_core::inflate::zlib(packed, 256)
                .ok()
                .filter(|bytes| bytes.len() as u64 == size)
                .ok_or_else(|| "the NativeAdrenoMetal oneios-target does not unpack".to_owned())?;
            target = Some(
                std::str::from_utf8(&bytes)
                    .map_err(|_| "the NativeAdrenoMetal oneios-target is not UTF-8".to_owned())?
                    .trim()
                    .to_owned(),
            );
        } else if name == PUBLISH {
            if publish {
                return Err("the gpuaddon contains duplicate NativeAdrenoMetal publish markers".into());
            }
            let bytes = xh_core::inflate::zlib(packed, 16)
                .ok()
                .filter(|bytes| bytes.len() as u64 == size)
                .ok_or_else(|| "the NativeAdrenoMetal publish marker does not unpack".to_owned())?;
            if bytes.as_slice() != b"1\n" {
                return Err("the NativeAdrenoMetal publish marker is not enabled".into());
            }
            publish = true;
        }
    }
    if at != blob.len() {
        return Err("the gpuaddon entries do not fill the declared length".into());
    }
    if !executable {
        return Err("the gpuaddon has no NativeAdrenoMetal executable".into());
    }
    if !publish {
        return Err("the gpuaddon has no enabled NativeAdrenoMetal publish marker".into());
    }
    match target.as_deref() {
        None => Err("the gpuaddon has no NativeAdrenoMetal oneios-target".into()),
        Some("iPhone10,3 20F66") => Ok(()),
        Some(other) => Err(format!("the NativeAdrenoMetal plug-in targets {other:?}, not iPhone10,3 20F66")),
    }
}

/// `ios addon`'s CDHash list (and `phone addon`'s reader of it): one 40-hex-digit CDHash per line, then the file's
/// name; blank lines and `#` comments are skipped.
fn parse_cdhashes(text: &[u8]) -> Result<Vec<[u8; 20]>, String> {
    let text = std::str::from_utf8(text).map_err(|_| "not a text list".to_string())?;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let word = line.split_whitespace().next().unwrap_or("");
        if word.len() != 40 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{line:?} is not a CDHash line (40 hex digits, then the file's name)"));
        }
        let mut hash = [0u8; 20];
        for (i, byte) in hash.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&word[2 * i..2 * i + 2], 16)
                .map_err(|_| format!("{word:?} is not a CDHash (hex digits only)"))?;
        }
        out.push(hash);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("test-addon-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("kernel")).unwrap();
        dir
    }

    /// A stand-in for `ios addon`'s sidecar: the magic, an entry count and the total, no files (this reader checks the
    /// magic and the total; the kernel checks the entries).
    fn addon() -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"NGPUADD2");
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&24u64.to_le_bytes());
        blob
    }

    #[test]
    fn cdhash_list() {
        let list = b"# ios addon, 2026-11-23\n\
4baf7dbb8b7faf1312e87d5b81ea830b5578b3fd addons/NativeAdrenoMetal.bundle/NativeAdrenoMetal\r\n\
\n\
1936cc1bb5954173c0b267f6c1d25d2684dee656 addons/NativeAdrenoMetal.bundle/libvulkan_freedreno.dylib\n";
        let hashes = parse_cdhashes(list).unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0][..4], [0x4b, 0xaf, 0x7d, 0xbb]);
        assert_eq!(hashes[1][19], 0x56);
        assert_eq!(parse_cdhashes(b"").unwrap().len(), 0);
        for bad in [
            &b"4baf7dbb8b7faf1312e87d5b81ea830b5578b3 addons/x\n"[..],
            b"zzaf7dbb8b7faf1312e87d5b81ea830b5578b3fd addons/x\n",
            b"\xff\xfe\n",
        ] {
            assert!(parse_cdhashes(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn the_runtime_addon() {
        // A runtime without one: nothing, quietly.
        let none = dir("none");
        assert!(runtime_addon(none.to_str().unwrap()).is_none());

        // The pair the packager places: the blob and its list.
        let rt = dir("pair");
        fs::write(rt.join("kernel/gpuaddon"), addon()).unwrap();
        fs::write(
            rt.join("kernel/gpuaddon.cdhashes"),
            "4baf7dbb8b7faf1312e87d5b81ea830b5578b3fd addons/NativeAdrenoMetal.bundle/NativeAdrenoMetal\n\
33247c78e0afb60610a6239991a859e342ac29f1 camera/MediaCapture/H9ISP.mediacapture\n",
        )
        .unwrap();
        let found = runtime_addon(rt.to_str().unwrap()).expect("the runtime's addon");
        assert_eq!(found.blob, addon());
        assert_eq!(found.cdhashes.len(), 2);
        // A trailing slash names the same folder.
        assert!(runtime_addon(&format!("{}/", rt.display())).is_some());

        // A blob without its trust list is not injected into the guest.
        fs::remove_file(rt.join("kernel/gpuaddon.cdhashes")).unwrap();
        assert!(runtime_addon(rt.to_str().unwrap()).is_none());
        // A list with a line that is not a CDHash: nothing is taken from it.
        fs::write(rt.join("kernel/gpuaddon.cdhashes"), "4baf addons/x\n").unwrap();
        assert!(runtime_addon(rt.to_str().unwrap()).is_none());

        // Not a boot addon at all: left alone.
        let bad = dir("bad");
        fs::write(bad.join("kernel/gpuaddon"), b"not an addon").unwrap();
        fs::write(bad.join("kernel/gpuaddon.cdhashes"), "").unwrap();
        assert!(runtime_addon(bad.to_str().unwrap()).is_none());
        // The magic, but a total that is not the file's length (cut short, or bytes after it): refused, as the loader
        // would refuse it.
        let cut = dir("cut");
        let mut blob = addon();
        blob.push(0);
        fs::write(cut.join("kernel/gpuaddon"), &blob).unwrap();
        fs::write(cut.join("kernel/gpuaddon.cdhashes"), "").unwrap();
        assert!(runtime_addon(cut.to_str().unwrap()).is_none());
        fs::write(cut.join("kernel/gpuaddon"), &addon()[..20]).unwrap();
        assert!(runtime_addon(cut.to_str().unwrap()).is_none());
        for d in [none, rt, bad, cut] {
            let _ = fs::remove_dir_all(d);
        }
    }
}
