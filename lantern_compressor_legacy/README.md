# Lantern Compressor

A character compressor with a drive stage, sibling of the Lantern synth.
Rust + [nih-plug](https://github.com/robbert-vdh/nih-plug), cross-compiled to
a Windows VST3 for Ableton Live running under Wine.

No custom GUI on purpose: with no editor, Ableton renders the parameters as
inline sliders right in the device chain, like a stock device.

## Controls

| Param | What it does |
| --- | --- |
| Threshold | Level where compression starts (-60..0 dB) |
| Ratio | 1:1 to 20:1 |
| Knee | Soft-knee width (0 = hard knee) |
| Attack | 0.05–100 ms |
| Release | 20–2000 ms (ignored while Auto Release is on) |
| Auto Release | Crest-factor adaptive release: fast on transients, slow on sustains |
| SC HPF | Sidechain high-pass so bass doesn't pump the whole mix (20–500 Hz) |
| Drive | Post-compression tanh soft clip, 0 dB = fully clean |
| Makeup | Manual output gain |
| Auto Makeup | Adds threshold/ratio-derived compensation on top of Makeup |
| Mix | Parallel (New York) compression |

Detection is stereo-linked peak, smoothed in the dB domain.

## Build & install

```sh
./build.sh                                  # cross-compile to Windows VST3
WINEPREFIX=~/.wine-ableton ./deploy.sh      # copy into Ableton's VST3 folder
```

Then rescan in Ableton (Preferences > Plug-Ins). Shows up as
"Lantern Compressor" (vendor "Alva").
