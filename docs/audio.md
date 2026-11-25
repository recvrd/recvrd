# Audio output

## Configuration

The `[audio]` section sets the PipeWire output on Linux. The defaults keep
the previous behavior.

| Key | Values | Default |
| --- | --- | --- |
| `sample_rate` | 8000 to 384000 Hz | 48000 |
| `format` | `f32`, `s32`, `s24` | `f32` |

```toml
[audio]
sample_rate = 96000
format = "s24"
```

An invalid sample rate or format stops the daemon at config load. The
`/api/status` response reports `audio_sample_rate` and `audio_format` when
the sink reports them. The connect log also names both values.

The non-Linux backend and the test sinks keep the default 48 kHz float
output.

## System format

recvrd requests a stream format. The device format belongs to the PipeWire
and WirePlumber ALSA node. To change the device, set the node properties in
the WirePlumber configuration. For example:

```text
monitor.alsa.rules = [
  {
    matches = [
      { node.name = "alsa_output.pci-0000_00_1f.3.analog-stereo" }
    ]
    actions = {
      update-props = {
        audio.format = "S24LE"
        audio.rate = 96000
      }
    }
  }
]
```

Restart the session manager after the change. Then check the result:

- `pw-top` shows the stream and device rates. The format column shows the
  stream format.
- `pw-dump` shows the node properties.
- `cat /proc/asound/card*/pcm*p/sub0/hw_params` shows the device rate and
  format while the device is open.

## Limits

- F32 is already 32-bit float. S32 is a different container, not more
  precision.
- YouTube Music streams are lossy Opus at 48 kHz. Upsampling one to 96 or
  192 kHz adds no detail.
- Volume below 100 percent rewrites samples in float. For a bit-perfect
  path, set the volume to 100 percent and use hardware volume.
