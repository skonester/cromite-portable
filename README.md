# Cromite Portable (Evolution Engine)

<div align="center">
  <img src="Logo.png" width="128" alt="Cromite Logo" />
  <br>

  [![Latest Release](https://img.shields.io/github/v/release/uazo/cromite?label=Cromite%20Version&color=008080&logo=chromium)](https://github.com/uazo/cromite/releases)
  [![Downloads](https://img.shields.io/github/downloads/uazo/cromite/total?label=Downloads&color=008080&logo=github)](https://github.com/uazo/cromite/releases)
  [![License](https://img.shields.io/github/license/uazo/cromite?color=008080)](https://github.com/uazo/cromite/blob/master/LICENSE)
  [![Platform](https://img.shields.io/badge/Platform-Windows-blue?logo=windows)](https://github.com/skonester/cromite)
  [![PowerShell](https://img.shields.io/badge/PowerShell-5.1%2B-blue?logo=powershell)](https://github.com/skonester/cromite)
  [![C#](https://img.shields.io/badge/C%23-9.0-blue?logo=c-sharp)](https://github.com/skonester/cromite)
  [![Last Commit](https://img.shields.io/github/last-commit/uazo/cromite?color=008080)](https://github.com/uazo/cromite/commits/master)

  <br>
  <a href="https://github.com/uazo/cromite/releases/latest">
    <img src="https://img.shields.io/badge/DOWNLOAD-CROMITE-orange?style=for-the-badge&logo=google-chrome&logoColor=white" alt="Download Cromite" />
  </a>
</div>

Cromite is a [Chromium](https://www.chromium.org/Home) fork based on [Bromite](https://github.com/bromite/bromite) with built-in support for ad blocking and an eye for privacy. This repository provides a **truly portable** Windows environment for Cromite, ensuring all data and configurations stay within the local folder.

---

## Project Structure & File Guide

Below is a breakdown of the core files and their purpose in this environment.

### Launchers & Executables
| File | Description |
| :--- | :--- |
| `Cromite Portable.exe` | **Native C# Launcher.** Located in the **root folder**, this high-performance entry point (6KB) directly executes Cromite from the `\app` directory with hardcoded privacy and portability flags. |
| `Cromite.bat` | A fallback batch script that launches Cromite directly from the `\app` folder with a full suite of privacy flags. |

### Management & Setup
| File | Description |
| :--- | :--- |
| `Update-Cromite.ps1` | **The Master Control Script.** Handles browser updates, privacy flag injection, environment cleanup, and can **rebuild the native launcher** from source. |
| `SetDefaultBrowser.bat` | Automates the process of registering this portable Cromite instance as your system's default browser. |
| `LauncherSource.cs` | The C# source code for the native launcher, provided for transparency and automated builds. |

### Configuration & Metadata
| File | Description |
| :--- | :--- |
| `app.ico` | The official Cromite icon, extracted from the browser binary and applied to the native launcher. |
| `portapp.json` | Metadata about the portable application, including versioning and publisher info. |
| `Logo.png` | A high-resolution 3D logo used for documentation and branding. |

---

## Key Features of this Environment
- **Zero System Footprint**: All user profiles, cache, and settings are strictly stored in a local `\data` directory.
- **Native C# Launcher**: Replaces heavy third-party launchers with a purpose-built, 6KB native binary for maximum speed and security.
- **Automated Branding**: `Update-Cromite.ps1` automatically extracts the official browser icon and applies it to your launcher.
- **Privacy Hardened**: Pre-configured with the "Evolution Engine" suite of 20+ privacy flags (no telemetry, no pings, no background networking).
- **Auto-Updating**: Integrated PowerShell management to keep your browser and launcher current with the latest `uazo/cromite` releases.
- **Dark Mode by Default**: Forced dark mode and WebUI dark mode enabled out of the box.

## Getting Started
1. **Initialize**: Run `Update-Cromite.ps1` and select `[1] Check for Updates` to download the browser.
2. **Setup**: Select `[3] Setup Portable Environment` to clean legacy files and prepare the structure.
3. **Build**: Select `[4] Rebuild Native Launcher` to compile your custom branded `.exe`.
4. **Launch**: Use `Cromite Portable.exe` to start browsing.

---
*Note: This project is a curated portable distribution and is not officially affiliated with the core Cromite/uazo team.*


