# Lantern 🏮

A polyphonic **subtractive synth with FM cross-modulation**, written in Rust with
[nih-plug](https://github.com/robbert-vdh/nih-plug). Exports **VST3** (for Ableton
Live running under Wine) and **CLAP**. Built with dubstep bass in mind.

## Why these choices

- **Rust, not C++** — via nih-plug, which exports modern VST3 + CLAP from one
  codebase. (Serum is C++/JUCE; we don't need that here.)
- **Cross-compiled to Windows** — Ableton Live under Wine is a *Windows* program,
  so it loads *Windows* plugins. We build `x86_64-pc-windows-gnu` with mingw and
  rustup's windows std.
- **VST3 is the target format** — Ableton Live 12 has no native CLAP support yet.

## Signal flow

```
Osc1 ─┐
      ├─(Osc2 phase-modulates Osc1 = FM)─► mix ─► resonant LP filter ─► amp env × vel ─┐
Osc2 ─┘                                          ▲                                      │
                                                 │                              Σ voices ─► drive (soft-clip) ─► gain ─► out
                              filter env + LFO ──┘
```

Dubstep cheat-sheet:
- **Reese bass** → two detuned saws (the default patch).
- **Wobble bass** → turn up **LFO Depth** (try a Square LFO for hard wub-wub).
- **Growl / grit** → **FM Amount** + **Drive**, high **Resonance**.

## Build

Requires the rustup toolchain set up in `~/.cargo` with the
`x86_64-pc-windows-gnu` target, plus the `x86_64-w64-mingw32-gcc` mingw linker.

```sh
./build.sh                 # -> target/bundled/*.vst3 and *.clap
```

## Install into Ableton (Wine)

Either copy the bundle into the prefix Ableton uses:

```sh
WINEPREFIX="$HOME/.wine-ableton" ./deploy.sh
```

…or add `target/bundled/` as a **VST3 Custom Folder** in Ableton:
**Preferences → Plug-Ins**, then **Rescan**.

## Roadmap

- Tempo-synced LFO (1/4, 1/8, 1/16 — proper dubstep wobble timing)
- egui GUI (knobs instead of the host-generic param list)
- Wavetable oscillator mode (the "Serum" path, for later)
- Per-voice drive / unison spread
