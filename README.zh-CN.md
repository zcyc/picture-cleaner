# Picture Cleaner

[English](README.md)

跨平台本地图片清理工具，支持 macOS 和 Windows。图片分析在本机完成，删除操作会先移入系统回收站。

## 功能

- **相似图片**：识别重复拍摄、裁剪或压缩后的近似照片。
- **手机截图**：根据文件名和手机屏幕比例筛选截图。
- **按时间**：默认使用照片拍摄时间（EXIF），也支持文件创建时间和修改时间。
- **键盘操作**：支持图片浏览、切换相似图片组、移入回收站和撤销删除。

相似图片会按组展示：同组使用 ←→ 浏览，↑↓ 切换到上一组或下一组。

## 快捷键

| 操作 | 快捷键 |
| --- | --- |
| 切换图片 | ← / → |
| 切换相似图片组 | ↑ / ↓（相似图片模式） |
| 移入回收站 | Delete / D / Backspace |
| 撤销最近删除 | Z / ⌘Z（macOS）/ Ctrl+Z（Windows） |

撤销支持在当前应用会话中连续回滚最近多次删除。

## 开发

需要 Node.js LTS、Rust stable 和 Tauri 所需的系统依赖。

```bash
npm ci
npm run tauri dev
```

## 构建、安装与签名

```bash
make build       # 构建当前平台应用包
make install     # 一键构建、签名、安装并启动当前平台版本
make sign        # 构建并签名当前平台应用包
```

### macOS

本地默认使用 ad-hoc 签名：

```bash
make install
```

如果已经安装 Apple Developer 证书，也可以指定签名身份：

```bash
APPLE_SIGNING_IDENTITY="Developer ID Application: Example (TEAMID)" make sign-macos
```

### Windows

本地会自动生成并复用当前用户的自签名代码证书：

```powershell
make install
```

需要先安装 Windows SDK 提供的 `signtool.exe`。如果有 `.pfx` 证书，也可以显式指定：

```powershell
$env:WINDOWS_CERTIFICATE_PASSWORD="证书密码"
make sign-windows WINDOWS_CERTIFICATE="C:\path\certificate.pfx"
```

## GitHub 自动发版

工作流位于 `.github/workflows/release.yml`。先同步以下文件中的版本号：

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

版本号一致后推送对应的 `v*` 标签，例如：

```bash
git tag v0.1.1
git push origin v0.1.1
```

Workflow 会自动构建：

- macOS arm64
- macOS x64
- Windows x64

## 当前限制

- 相似图片扫描目前采用两两比较，超大图库可能需要更长时间。
- 撤销记录是临时会话数据，不跨应用重启保留。
