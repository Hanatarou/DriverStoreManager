# DriverStore Manager — Review, Back Up and Clean the Windows Driver Store

## 🤖 A Note on AI Usage

I built this project alone, as a personal project, with substantial help from AI tools — mainly Claude, and also DeepSeek and Qwen. I believe knowledge only survives past us if it's shared, and that's the spirit behind releasing this for free.

I did this on my own time and dime, covering all costs myself, without asking anyone for donations.

Just as I respect opinions against the use of AI, I expect the use of AI here — as a tool that helped me build this project — to be respected in return. Disrespect toward this work, toward me, or toward anyone else involved will not be tolerated.

All AI-generated content is reviewed and validated by me before being committed — I stand behind every decision to include code in this repository, regardless of how it was originally written. This project is still provided as-is, under the MIT license below, with no warranty of any kind.

If you're uncomfortable with AI-assisted code for any reason, you are under no obligation to use, contribute to, or engage with this project. No hard feelings — just move on.

For everyone else: bug reports and PRs are evaluated on their merits (does it work, is it correct), not on how the code was produced.

> A notice about AI is also shown inside the program, in **Help > About**.

## 🙏 Inspired by Driver Store Explorer

DriverStore Manager exists because of **[Driver Store Explorer (RAPR)](https://github.com/lostindark/DriverStoreExplorer)** by **lostindark** and its contributors. That project showed how useful a graphical view of the Driver Store is, and it defined the ideas this program builds on: listing third-party packages with their devices, finding old versions of the same driver, and keeping a safe path to remove them. Many thanks to everyone who built and maintains it. If Driver Store Explorer does what you need, it is a mature, widely used tool and a great choice.

DriverStore Manager is an **independent re-implementation in Rust**, started from my own PowerShell script, with a different goal: be as careful as possible when something is deleted. No source code from Driver Store Explorer was copied; the ideas and a few behaviors (such as never touching `ntprint.inf` automatically) were learned from it, and credited here and in **Help > About**. Driver Store Explorer is licensed under GPL-2.0.

## 🎯 What is DriverStore Manager?

A small, native Windows program that lists every **third-party driver package (`oemNN.inf`)** in the Driver Store and tells you, for each one:

- whether it is **Old** (a newer package of the same driver exists, or an identical duplicate is kept instead),
- whether it needs **Review** (version and date disagree about which package is newer),
- whether it is **In use** and by which devices — plugged in or not.

You can then check the old ones, back them up, and remove them; add packages to the store (optionally installing them on matching devices); export packages as a backup; and export the list to CSV or JSON.

- ✅ Single `.exe`, plain Win32, no .NET and no PowerShell
- ✅ English only, on purpose (see below)
- ✅ Backups are verified file by file (SHA-256); a package whose backup failed is never removed
- ✅ The list is re-verified against the Driver Store right before removing

## ⭐ Where it tries to be better

Every item below is something the program does; the classification, selection, device-mapping, settings and folder-safety rules have unit tests. These are design choices, not a claim that this tool is better for everyone.

### Safety first
- 🛡️ **Verified backup before every removal:** every file of the copy must have the same name, size and **SHA-256** as the original (a size check alone misses a damaged copy). If the copy is not identical, that package is **not** removed.
- ♻️ **Restore backup:** every backup and export folder gets a `manifest.txt` (the devices that used each package and the SHA-256 of every file). **Drivers > Restore backup…** checks each file against it and installs only the packages that are intact. **Drivers > Verify backup…** does the same check without restoring anything.
- 🔁 **Stale-list protection:** Windows reuses `oemNN` numbers, so just before removing, the checked packages are compared with the live Driver Store (INF name, version, date). If anything changed, **nothing** is removed and the list refreshes.
- ✅ **Post-removal check:** the store is read again, and a package that `pnputil` reported as removed but is still there is reported as a failure.
- ❓ **Confirmation defaults to No,** with specific warnings: still in use, not old, unused but latest, boot-critical.
- 🧭 **"Review" status:** when date and version disagree, the program does not guess. It never selects those packages automatically.
- 🖨️ **`ntprint.inf` (print spooler) is never checked automatically.**
- 🔒 **Link-safe folders:** the program runs elevated, so it refuses a `log` or `backup` folder that is a junction or symbolic link, and writes its settings file by replacing it, not by following a link.
- 🧱 **Defensive reading:** a damaged settings file falls back to defaults, and Driver Store data that does not look like a driver package stops the load instead of showing wrong packages.

### Correctness
- 🧩 **Extension INFs count as "in use":** a package used through a device's extension INF is no longer shown as unused.
- 👯 **Duplicates:** identical packages (same version and date) — one stays (the one a device uses, otherwise the lowest number), the others are marked as duplicates.
- 🌐 **Windows-language independent:** success is judged by the `pnputil` exit code (0, or 3010 = restart required), never by its text, so it behaves the same on every language edition.
- 📏 **Unreadable file in a package?** The size is shown as `12 MB+` and the reason goes to the log; the list still loads. (Backups stay strict.)
- 🗓️ Dates and times in logs and file names always use the same format (`yyyy-MM-dd HH:mm:ss` in logs, `yyyyMMdd_HHmmss` in file names), whatever the Windows calendar.

### Practical
- ⚡ **No PowerShell, no WMI:** packages come from the Windows Driver Store library (`drvstore.dll`, the one `pnputil` uses) and devices from the Windows Configuration Manager. It does not need DISM, so it also works where DISM is broken or stripped down (for example Windows PE).
- 🪟 The window stays responsive during long copies and `pnputil` runs, and it cannot be closed in the middle of an operation.
- 💾 Remembers your options, grouping, sorting, column widths and window position (`DriverStoreManager.ini`).
- 💽 **Offline Windows images:** **File > Open offline Windows image…** manages the third-party drivers of a Windows on another disk (recovery, Windows PE): list and export read the image through `drvstore.dll`; add, remove and restore go through the DISM API. Offline there are no devices, so *In use* is shown as *Unknown*, *Check unused packages* and *Add and install* are not available, and the backups go to a folder you choose.
- 📤 CSV (opens correctly in Excel, UTF-8 with BOM) and JSON export of what is shown: the same columns, titles and cell text as the window.
- 🔏 **Signature** (the class Windows gives it: Logo Premium, Logo Standard, WHQL, Inbox, Unclassified, Authenticode, Unsigned…), **Signer**, **Install date (UTC)**, **Extension ID**, **Driver files** (the count and the first five names), **Device ID** and **Driver path** columns, all sortable and searchable.
- 💬 Hover a cell that is cut off to see all of its text.
- 🔌 **View > Show only packages used only by disconnected devices:** drivers whose hardware is not plugged in, good candidates for cleanup.
- ⚠️ **View > Show only packages used by devices with a problem:** packages bound to a plugged-in device that Device Manager reports with a problem code (shown as `problem code NN` in the *Devices* column).

## 🔍 What it does *not* do (yet)

Being honest about the difference with Driver Store Explorer:

- ❌ **English only.** This is intentional: driver operations are risky enough without adding translation problems.
- ❌ No automatic updates.

## 📦 Installation

There is no installer. Copy `DriverStoreManager.exe` to a folder **that only administrators can write to** (for example `C:\Program Files\DriverStore Manager\`) and run it. Windows will ask for administrator rights.

The program keeps these next to the `.exe`:

| Item | Location |
|---|---|
| Log, one file per run | `.\log\DriverStoreManager_<start time>.log` |
| Backups made before removal | `.\backup\<time of removal>\<class>\<inf>_<version>_<oemNN>\` and `.\backup\<time of removal>\manifest.txt` |
| Settings | `.\DriverStoreManager.ini` |
| Exports | `<folder you choose>\DriverStoreManager_export_<time>\` |

> Installing in a folder that normal users can write to is possible but not recommended: the program runs as administrator and writes there.

### Requirements
- Windows 10 version 1607 (build 14393) or later, 64-bit
- Administrator rights

## 🚀 Usage

### Menus

- **File:** Refresh (`F5`), Export list (`Ctrl+E`), Open offline Windows image, Return to the running Windows, open log and backup folders.
- **Drivers:** Add driver package (`Ctrl+N`), Add and install (`Ctrl+Shift+N`), Export checked/all, Restore backup, Verify backup, Remove checked.
- **Select:**
  - *Check old packages (unused only - safe)* — the recommended starting point.
  - *Check old packages (including in use)*, *Check unused packages* (any age), check/uncheck shown, invert, uncheck all.
- **View:** Group by (class, provider, INF, usage, status), Show only old packages, Show only packages used only by disconnected devices, Show only packages used by devices with a problem, Go to the filter box (`Ctrl+F`).
- **Options:** Back up before removing (on by default), include boot-critical packages in automatic selections (off).
- **Right-click on a row:** check/uncheck a group, remove or export the selected rows, open device properties, open the package folder, copy its path, copy the text of the clicked cell, copy the selected rows (tab-separated, with the column titles).

### Row colors (never the only signal — the *In use* and *Status* columns say the same in text)

| Color | Meaning |
|---|---|
| Light blue | Old, no device uses it |
| Light yellow | Old, but a device still uses it |
| Light gray | Review: version and date disagree |
| None | Latest |

### A cautious first run
1. Start the program and just **look at the list**; nothing changes until you confirm a removal.
2. Use **Select > Check old packages (unused only - safe)**, read what is checked, and uncheck anything you are unsure about.
3. Keep **Back up packages before removing** on.
4. Remove, then restart if the program asks you to.

To undo a removal, use **Drivers > Restore backup…** and pick its folder in `backup` (or an export folder): the files are checked against the manifest first. **Drivers > Add driver package…** also works with any folder that has `.inf` files, including backups made before the manifest existed.

## 🔧 How it works

- **Reading:** `drvstore.dll` (the third-party packages of the running Windows or of an offline image, with their provider, version, date, signer, extension ID and install date) and `cfgmgr32.dll` (all devices, including ones not plugged in, with their driver INF and extension INFs). The library is not documented, so it is loaded from the System32 folder only, every function is looked up by name, every value is checked against its type and size, and reading stops with an error if the data does not look right.
- **Changing:** only `pnputil.exe`, from the Windows system folder:
  - remove: `pnputil /delete-driver oemNN.inf /uninstall` (a package still in use is removed the same way; `/force` is not used because `pnputil` ignores it together with `/uninstall`),
  - add: `pnputil /add-driver <folder>\*.inf /subdirs [/install]`,
  - offline images: `DismRemoveDriver` and `DismAddDriver` (pnputil only works on the running Windows; DISM is only needed for this),
  - after a removal: `pnputil /scan-devices`.
- **Old:** another package with the same class, provider, INF name and extension ID has a date and a version that are both not lower, and at least one higher; or an identical package is kept instead.

## 🛠️ Build

Rust 2021, MSVC toolchain, x64. The icon and version information are compiled with `rc.exe` (installed with Visual Studio / Build Tools).

```
cargo build --release
-> target\release\DriverStoreManager.exe   (manifest: requireAdministrator)
```

Smaller executable (nightly, standard library rebuilt for size):

```
rustup toolchain install nightly -c rust-src
set RUSTFLAGS=-Zunstable-options -Cpanic=immediate-abort
cargo +nightly build --release --target x86_64-pc-windows-msvc -Z build-std=std,panic_abort -Z build-std-features=optimize_for_size
-> target\x86_64-pc-windows-msvc\release\DriverStoreManager.exe
```

With `panic=abort` an unexpected internal error ends the program without a message; normal errors are always shown and logged.

### Tests

```
cargo test
```

The unit tests cover the classification rules, device mapping, sorting and grouping, the Driver Store structure layouts and value checks, settings, CSV/JSON output, folder safety, file copying, SHA-256 (against the published test vectors) and the backup manifest. They do not replace trying the program on a real machine.

To check the code on a machine without the Windows resource compiler, set `DSM_SKIP_RESOURCES=1` (the `.exe` then has no icon).

## ⚠️ Warning

Removing drivers can stop devices from working and, for boot-critical drivers, can prevent Windows from starting. Use it on a computer where you can recover, keep the backups the program makes, and read the confirmation dialogs.

## 📚 Additional Info
- **Driver Store Explorer (inspiration):** https://github.com/lostindark/DriverStoreExplorer (GPL-2.0)
- **Author:** Bruno Eduardo, https://github.com/Hanatarou
- **License:** MIT
