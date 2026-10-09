# yyeditor / yy Applications

English | [日本語](README.ja.md)

<img src="apps/yyeditor/res/yyeditor-256.png" alt="yyeditor icon" width="96">

A collection of Windows applications written in Rust for text editing, terminal connections, file transfer, spreadsheet processing, shared-folder management, web browsing, and clipboard reuse. The applications include tools for working with huge files, Japanese character encodings, and mainframe data.

This repository contains the following applications and their shared libraries.

| Application | Purpose |
| --- | --- |
| **yyeditor** | A text editor for huge files, Japanese text, CSV, and binary data |
| **yyterm** | A terminal for local shells, SSH sessions, and IBM 3270 connections |
| **yysftp** | File and folder transfer over SFTP / SCP |
| **yysheet** | A spreadsheet for large datasets, formulas, and COBOL fixed-length data |
| **yyclip** | A tray application for clipboard history, text templates, and favorite files and folders |
| **yyfilemanager** | Synchronization, search, duplicate detection, and version cleanup for local and shared folders |
| **yybrowser** | A tabbed browser with its own proxy settings, ad blocking, and bookmarks |
| **yy-agent** | A Linux helper for file operations on SSH hosts |

For development prerequisites, builds, tests, and internal architecture, see [DEVELOPER.md](DEVELOPER.md).

## yyeditor — Text Editor

View and edit logs, source code, Japanese documents, CSV, and fixed-length records in one application. Huge files are opened using memory mapping, while tasks such as line counting run in the background.

- **Editing**: Multiple document tabs, IME input, rectangular selections, multiple cursors, undo / redo, text transformations, and duplicate-line removal. Undo remains available after saving.
- **Search**: Incremental search, regular expressions, replacement, selection of all matches, folder search (Grep), and navigation to results.
- **Comparison**: Two open documents displayed side by side with aligned lines, highlighted differences, and synchronized scrolling.
- **Encodings**: UTF-8 / UTF-16 / UTF-32, CP932, Shift_JIS, Shift_JIS-2004, EUC-JP, EUC-JIS-2004, ISO-2022-JP, and various EBCDIC encodings. Supports automatic detection, reopening with a specified encoding, and encoding, BOM, and line-ending conversion when saving.
- **CSV / TSV**: Aligned columns, navigation between cells, column insertion and deletion, and delimiter conversion. Handles quoted fields and embedded line breaks.
- **Binary and fixed-length data**: Hexadecimal display and editing, Unicode code-point editing, and fixed-length views with byte-position rulers.
- **Code and document display**: Syntax highlighting for Rust, C / C++, Python, JavaScript / TypeScript, COBOL, JCL, and more; bracket matching and comment toggling. Markdown / HTML previews can display Mermaid diagrams, KaTeX formulas, and d3 visualizations.
- **Workspaces**: Multiple root folders, file operations, recent files, and bookmarks. Git support includes changes and diffs, staging, commits, branch operations, fetch, pull, and push.
- **Remote editing**: Open and save files on Linux SSH hosts, with confirmation when a file has changed externally. Remote workspaces and Git operations are also supported.

EBCDIC support includes IBM-930 / 939 / 1390 / 1399, with newline-delimited and fixed-length records. External mapping tables can add vendor-specific Japanese encodings and user-defined characters. JEF, KEIS, and JIPS mapping tables are not bundled.

See the [yyeditor help](crates/yy-win/help/help.md) (Japanese), or press F1 in the application, for detailed instructions.

## yyterm — Terminal and 3270 Emulator

Open local shells, SSH sessions, and 3270 terminals in separate tabs.

- Local shells use Windows ConPTY. The default shell selection tries PowerShell 7, then Windows PowerShell, then Command Prompt.
- Supports SSH connections, jump hosts, proxies, and local, remote, and dynamic port forwarding.
- Supports full-width and combining characters, 256-color and 24-bit color, alternate screens, mouse input, bracketed paste, and IME input.
- Provides scrollback, copy and paste, and clickable URLs and file paths. File links open the editor, and folders in SSH tabs can be opened in yysftp.
- Shared workspaces let you open a terminal in a selected local or remote folder.

**IBM 3270** support includes TN3270 / TN3270E, models 2–5, and Japanese CCSIDs 930 / 939 / 1390 / 1399. Connections can use direct TCP, TLS, or SSH. Features include custom key bindings, an on-screen keypad, Rhai macros and operation recording, IND$FILE GET / PUT, 3287 printer output, communication traces, and screen replay.

## yysftp — File Transfer

An SFTP / SCP client with a folder tree and file list for browsing SSH hosts.

- Upload files and folders by dragging them from File Explorer, and download selected items.
- Create folders, rename items, and delete remote files.
- Manage transfer queues with progress and speed indicators, pause and resume, and automatic reconnection. Transfer state is recorded so transfers can resume after an application restart.
- Write to temporary destination names and rename files after verification. When using the agent, transfers also verify the entire file with SHA-256.
- View transfer logs in the application and in a log file.

By default, transfers use the host's SFTP / SCP facilities without deploying yy-agent. Agent mode can be enabled in settings.

## yysheet — Spreadsheet for Large Datasets

A spreadsheet application that reads and writes CSV / TSV, its native `.yys` format, and fixed-length files defined by COBOL layouts. Data is stored by column and loaded as needed to handle large numbers of rows.

- **Table editing**: Cell and formula-bar input, IME, multiple sheets, copy and paste, row and column insertion and deletion, undo / redo, fill handles, search and replace, and transposed paste.
- **Data processing**: Filtering by multiple conditions, sorting by multiple keys, duplicate removal, and column types. Manually entered or pasted ranges can be converted to tables for filtering and sorting.
- **Formulas**: Arithmetic, cell references, `SUM`, `COUNT`, `SUMIFS`, `COUNTIFS`, `XLOOKUP`, `ROW`, `AND`, `OR`, `IFS`, `POWER`, string functions, and more. Supports dynamic-array spills and dependency-based recalculation.
- **Formatting**: Excel-compatible number, date, time, and Japanese-era formats, text colors, fills, bold and italic text, alignment, and borders.
- **Delimited data**: Import with inferred encoding, delimiter, headers, and column types. Supports quoted fields and embedded line breaks, as well as delimiters based on control characters such as US / RS.
- **Fixed-length data**: Display COBOL copybook fields as columns. Edit text, zoned decimal, packed decimal (COMP-3), binary numbers, and other field types, and export in MS932 / EBCDIC. Supports multiple layouts, layout catalogs, and COBOL MOVE-style conversions.
- **Remote files**: Open and save spreadsheet data on SSH hosts through shared workspaces. Uses SFTP by default, with optional agent support.

The native `.yys` format saves changes by appending modified data. The application implements a subset of Excel formulas and display formats; its supported file formats are `.yys`, delimited data, and fixed-length data.

See the [yysheet help](crates/yy-win/help/sheet.md) (Japanese), or press F1 in the application, for instructions and the function reference.

## yyclip — Clipboard History and Launcher

A tray application that saves copied text for later reuse. It runs independently of the other yy applications.

- Saves text up to 1 MiB per entry. History persists after the application exits.
- Press and release Control twice within 500 milliseconds to open the picker, or click the tray icon. The first item is selected and ready for keyboard navigation when the picker opens.
- Switch between Clipboard, Templates, and Favorites using arrow keys or Ctrl+Tab.
- Press Enter or double-click a history entry or template to put it back on the clipboard.
- Register, edit, and delete up to 20 multiline templates. Optional notes describe their purpose; only the template body is copied to the clipboard.
- Register up to 20 favorite files and folders in total. Files open in their default application, and folders open in File Explorer.

Clipboard content without text, such as images or file lists, is not saved. See the [yyclip README](apps/yyclip/README.md) (Japanese) for usage and storage locations.

## yyfilemanager — Shared-Folder Synchronization, Search, and Cleanup

Transfer and organize files in local folders and Windows network shares. It works with shares accessible through File Explorer, while yysftp handles file transfers over SSH.

- **Synchronization**: One-way update or mirror synchronization from source to destination. Review the plan before execution; files also changed at the destination are treated as conflicts. Swap the direction to synchronize a share to a local folder.
- **Resume and delta transfer**: Resume after disconnection or application exit, transfer only changed blocks of large files, verify transfers, and optionally copy access permissions.
- **Search**: Combine names, regular expressions, extensions, sizes, dates, attributes, and content. Detects Japanese encodings and can search Office documents (docx / xlsx / pptx) and PDFs containing text.
- **Indexes and saved searches**: Reuse file catalogs, hashes, and full-text indexes. Save search conditions, compact indexes, and limit background CPU and memory usage.
- **Duplicate and version detection**: Find duplicates by content hashes and suggest the latest version using names and timestamps. Restrict candidates by extension or name regex, and exclude folders or files.
- **Deletion and recovery**: Select items in a review list, then move local files to the Recycle Bin and shared files to quarantine folders. Restore quarantined files from operation records, and clean up files beyond the retention period.
- **Scheduled synchronization**: Run saved jobs without a window using `--sync`, and register them with Windows Task Scheduler.

Version detection is a heuristic. Review synchronization and deletion plans before execution. See the [yyfilemanager help](crates/yy-win/help/filemanager.md) (Japanese) for details.

## yybrowser — Tabbed Browser with Configurable Connection Routes

A Microsoft Edge WebView2 browser with connection settings independent of the OS. Profiles let you use corporate networks, SSH dynamic port forwarding, and testing proxies separately.

- **Connection settings**: Choose OS settings, direct connections, HTTP / HTTPS / SOCKS proxies, or PAC. Rules can select different routes for individual domains.
- **Profiles**: Separate proxy settings, cookies, caches, and login state. Open different profiles in separate windows.
- **Testing connections**: Map hostnames to specific IP addresses and ports, and accept development certificates only for the configured host and certificate fingerprint.
- **Ad blocking**: Block advertising and tracking requests using EasyList, EasyPrivacy, AdGuard Japanese filters, and other lists. Supports automatic updates, additional lists, site exceptions, and per-profile enable / disable settings.
- **Bookmarks**: Add pages using the star button or Ctrl+D. Organize folders, edit and reorder entries, and import or export HTML compatible with other browsers. Bookmarks are shared across profiles.
- **Downloads**: Save to the Downloads folder or a chosen folder (or ask each time), with progress, pause / resume / cancel, and a download list (Ctrl+J). A download whose content already exists in the folder (same SHA-256) is discarded instead of being saved under another name.
- **History**: Search and reopen visited pages (Ctrl+H), and clear browsing data by period and kind — history, download history, cache, cookies and site data, autofill (Ctrl+Shift+Del). History is kept per profile.
- **Search engine**: Choose Bing, Google, Yahoo! JAPAN, DuckDuckGo, Brave Search, Startpage, or a custom URL for address-bar searches.
- **Browsing**: Tabs, reopening closed tabs, page search, zoom, printing, full-screen mode, and developer tools.

See the [yybrowser help](crates/yy-win/help/browser.md) (Japanese) for usage and connection settings.

## Integration and Requirements

The GUI applications run on Windows. Local shells in yyterm require ConPTY from Windows 10 version 1809 or later. The yyeditor preview and yybrowser require the Microsoft Edge WebView2 runtime.

yyeditor, yyterm, yysftp, and yysheet share settings and SSH connection information, and can share working folders through `*.yyworkspace` files. Place related executables in the same folder to enable actions such as opening a terminal from the editor. The default monospaced font, UDEV Gothic, is bundled.

The SSH client is built in and does not invoke local `ssh.exe`. It supports public-key, password, and keyboard-interactive authentication, host-key verification, jump hosts, and HTTP / SOCKS proxies. Passwords and private-key passphrases can optionally be stored in Windows Credential Manager.

yy-agent supports Linux x86_64 / aarch64. For remote editing, yyeditor deploys the matching binary from the `agents/` folder beside its executable. SSH shell sessions in yyterm work without an agent.

Shared settings are primarily stored in `%APPDATA%\yyeditor\config.toml`. yyclip stores its history, templates, and favorites under `%APPDATA%\yyclip\`.

yyfilemanager stores jobs, indexes, and operation records in `filemanager/` under the shared settings folder. yybrowser stores profiles in `browser.toml` in that folder, and per-profile browsing data under `%LOCALAPPDATA%\yyeditor\yybrowser\`.

## Current Limitations

- yyeditor downloads the entire remote file before opening it. Saving through `sudo` is not supported.
- Large documents in encodings other than UTF-8 cannot be edited or saved until conversion completes.
- yyeditor's CSV mode does not provide frozen headers, sorting, or filtering. Use yysheet for table processing.
- yyeditor previews documents up to 8 MiB. HTML scripts and d3 code blocks are executed.
- yyeditor's fixed-length view provides byte-level display and editing. Use yysheet for editing based on COBOL field types.
- yyfilemanager does not merge changes through bidirectional synchronization. Content search does not cover image-only or encrypted PDFs.
- yybrowser connection profiles apply per window. Some ad-blocking rules, including uBlock Origin-specific script injection, are not supported.

## License

Distributed under the [GNU General Public License v3 or later](COPYING) (GPL-3.0-or-later), with an [additional permission](COPYING.EXCEPTION) under GPL section 7 for linking and distributing the Microsoft Edge WebView2 Loader.

Bundled third-party materials retain their own licenses.

| Material | License / reference |
| --- | --- |
| UDEV Gothic | [SIL Open Font License 1.1](crates/yy-win/fonts/LICENSE-UDEVGothic.txt) |
| Mermaid / KaTeX | MIT; licenses and versions are included in the [preview assets](crates/yy-preview/assets/) |
| d3 | [ISC](crates/yy-preview/assets/LICENSE-d3.txt) |
| EBCDIC conversion tables | Unicode License V3; generated from ICU mappings |
| Microsoft Edge WebView2 Loader | Microsoft WebView2 SDK license; see the additional permission above |

Ad blocking uses Brave's adblock crate (MPL-2.0). Refer to Cargo dependency metadata for the licenses of other dependencies.
