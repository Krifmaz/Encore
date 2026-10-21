# Example tone

An example source plugin for Encore, written with the plugin SDK. It "searches" two short albums of tones by
*The Example Tones* and "downloads" the one picked by writing a sine tone as a FLAC file. No network, no
account, nothing to install besides Python 3.11 or newer.

- `plugin.toml`: the manifest.
- `plugin.py`: the program (search, download, status, test, and a small FLAC writer).
- `encore_plugin.py`: a copy of the SDK (`plugins/sdk/encore_plugin.py`), as every plugin carries one.

Try it from the repository's root:

```bash
python3 plugins/tools/plugin-check.py docs/plugins/example-tone --data /tmp/tone-data \
    --track "The Example Tones|Concert A" --download 1 --folder /tmp/tone-files
```

Install it in Encore: zip the folder (`cd docs/plugins && python3 -m zipfile -c example-tone.zip example-tone/`)
and use **Upload plugin** on Settings → Plugins, or **Install from URL** with
`Krifmaz/Encore/docs/plugins/example-tone@main`. Then search for "tone" in the download picker.

[docs/plugins/README.md](../README.md) walks through it.
