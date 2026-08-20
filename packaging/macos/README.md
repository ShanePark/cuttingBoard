# macOS packaging

Run the build on macOS from the repository root:

```sh
./scripts/build-macos-app.sh
```

When `.venv` does not exist, the build delegates to `scripts/install-macos.sh`
to install Homebrew `python-tk@3.13` and create a Python 3.13 environment with
Tk support. It then installs the build dependencies, generates an `.icns` file
from `assets/app-icon-source.png`, and produces these artifacts:

- `dist/Cutting Board.app`
- `dist/Cutting-Board-<version>-macos-<architecture>.zip`

The application and ZIP are unsigned and not notarized. The build architecture
matches the Python interpreter used to create `.venv`; build once on each target
architecture when distributing native Intel and Apple Silicon artifacts.
The icon step uses macOS `sips` and `iconutil`, with a small compatible ICNS
packer as a fallback for macOS versions that reject otherwise valid iconsets.

## Homebrew Cask

Copy `cutting-board.rb.in` to `Casks/cutting-board.rb` in a personal tap and
replace the three placeholders:

- `@VERSION@`: the application version printed by the build
- `@SHA256@`: the ZIP checksum printed by the build
- `@URL@`: a public HTTPS URL for that ZIP

For a local-only Cask test, `@URL@` can be an absolute `file://` URL to the ZIP.
The template describes one architecture-specific artifact. Add Homebrew's
architecture-specific `arch` and `sha256` declarations when both artifacts are
published under one Cask.
