# Contributing to OneiOS

PRs welcome — especially ports to other phones. This file is the whole process; [CLAUDE.md](CLAUDE.md) has the
working rules agents and humans both follow, [docs/forks.md](docs/forks.md) maps every known fork.

## How to send a PR

1. Fork, branch from `main`, keep the branch focused (one device or one fix).
2. Commit with a pathspec (`git commit -- <your paths>`), never `git add -A`.
3. Every change updates its docs in the same commit — [CLAUDE.md](CLAUDE.md) has the change-to-doc table
   (tl;dr: code + its README/section + tests, together).
4. Prove it: host tests (`runtime/host-tests.sh` and friends), and a PC boot under `qemu-aarch64` where it matters.
   Say what ran and what didn't — "not run on the phone" is honest, silence is not.
5. Open the PR against `akshaynexus/OneiOS` `main`. Draft is fine while the phone proof is pending; mark what the
   phone run must show.

Hard nos: no Apple files in git (IPSWs, kernelcaches, kexts, `kernel.kc` — fetch scripts pull them to `out/` and
`inputs/`, both git-igned); no new Python (use `oneios-tool`, the Rust tool, or plain POSIX shell); no changing
the S24 Ultra defaults for another device's sake.

## Add a device (vibe-coding welcome)

You don't need to understand the whole stack. Follow the probe record:

1. **Probe first.** Run `tools/probe/s25-probe.sh` on the new phone (root, One UI running; it only reads). It writes
   a tarball of facts: CPU/topology, KGSL, panel, input devices, HALs, SELinux. No probe output, no port.
2. **Backends before drivers.** Add the phone to the hardware-backend stand-ins (`tools/oneios-tool/tests/phone/hw-backend.sh`
   pattern: samsung vs treble) and make them pass. If a bridge needs the phone's HAL, extend the backend behind
   `hw-backend-<bridge>` — never fork the bridge.
3. **Per-device records, not forks.** Add a `docs/<device>.md` port record (copy `docs/s25.md`: F/I/U-marked device
   table, verdict table, probe→decision matrix) and a host profile next to `profiles/` if the build needs one.
   Device-specific numbers (panel mode, sensor paths) live in config, not in `#ifdef`s.
4. **GPU last.** The display/GPU path is the least portable part. Bring the `virtio_gpu` golden tests with you
   (template: the Xclipse adaptation), keep Adreno specifics behind the backend, and expect this step to need
   a maintainer's eye.
5. **Prove it like the S24.** SpringBoard on the panel, touch, return to One UI intact. Post the session log lines
   that prove each one (the `oneios-*` lines, not "it felt fast").
6. **Track it.** Put your fork and phone on the iOSOnAnd tracker (<https://hackios.appaiproxy.com>), where every
   port's per-area status is public. With Claude Code in this repo, run `/track OWNER/REPO DEVICE-SLUG` once
   (`.claude/skills/track/SKILL.md`): it signs you in with a GitHub gist proof (no token is sent), claims your repo
   for your phone and records each area from your docs and logs. After that, the agent updates an area's status
   itself whenever a phone boot proves or breaks it (CLAUDE.md, "The iOSOnAnd tracker"). By hand, the same steps
   are in that skill file. Claim only what your logs show.

Stuck? Open a draft PR early with the probe tarball's summary — a half-port with facts beats a finished port
with guesses. And read [docs/forks.md](docs/forks.md) first: someone may already have your device halfway done.
