# Picture Cleaner

[简体中文](README.zh-CN.md)

A cross-platform local photo cleaner for macOS and Windows. Image analysis runs locally, and deleted files are moved to the system Trash or Recycle Bin.

## Features

- **Similar photos**: Finds burst shots and visually similar photos created by cropping or compression.
- **Screenshots**: Filters screenshots using filenames and phone screen aspect ratios.
- **Date filtering**: Uses EXIF capture time by default, with file creation and modification time options.
- **Keyboard workflow**: Browse photos, switch similar-photo groups, move files to Trash, and undo deletions.

Similar photos are grouped together. Use ←→ to browse within a group and ↑↓ to switch groups.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Switch photos | ← / → |
| Switch similar-photo groups | ↑ / ↓ (similar-photo mode) |
| Move to Trash | Delete / D / Backspace |
| Undo recent deletion | Z / ⌘Z (macOS) / Ctrl+Z (Windows) |

Undo supports multiple recent deletions during the current application session.

## Development

Requires Node.js LTS, Rust stable, and the system dependencies required by Tauri.

```bash
npm ci
npm run tauri dev
```

## Build, Install, and Sign

```bash
make build       # Build the application package for the current OS
make install     # Build, sign, install, and launch for the current OS
make sign        # Build and sign for the current OS
```

### macOS

Local builds use ad-hoc signing by default:

```bash
make install
```

To use an installed Apple Developer certificate instead:

```bash
APPLE_SIGNING_IDENTITY="Developer ID Application: Example (TEAMID)" make sign-macos
```

### Windows

Local builds generate and reuse a self-signed code-signing certificate for the current user:

```powershell
make install
```

Install the Windows SDK to provide `signtool.exe`. To use a `.pfx` certificate instead:

```powershell
$env:WINDOWS_CERTIFICATE_PASSWORD="certificate password"
make sign-windows WINDOWS_CERTIFICATE="C:\path\certificate.pfx"
```

## GitHub Releases

The workflow is defined in `.github/workflows/release.yml`. Keep the versions in sync in:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Push a matching `v*` tag to create a release:

```bash
git tag v0.1.1
git push origin v0.1.1
```

The workflow builds:

- macOS arm64
- macOS x64
- Windows x64

## Limitations

- Similar-photo scanning currently compares images pairwise and may be slow for very large libraries.
- Undo history is temporary and is not preserved across application restarts.
