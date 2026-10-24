# yyeditor / YY applications

English | [日本語](README.ja.md)

<img src="apps/yyeditor/res/yyeditor-256.png" alt="yyeditor" width="96">

Windows tools for editing, terminals, transfer, spreadsheets, diagrams, databases and comparison.

| Application | Current features | Guide |
| --- | --- | --- |
| **yyeditor** | Huge text, Japanese/EBCDIC, CSV, fixed records, binary and live Markdown | [Guide](crates/yy-win/help/help.md) |
| **yyterm** | Local/SSH/3270 terminals, tmux/screen, macros and per-tab logs | [Guide](crates/yy-win/help/help.md) |
| **yysftp** | SFTP/SCP, resumable transfers, SSH gateways and per-host listing commands | [Guide](demo/yysftp/README.md) |
| **yysheet** | Large CSV, formulas, COBOL fixed records and layout/record-length inference | [Guide](crates/yy-win/help/sheet.md) |
| **yyclip** | Clipboard history, templates, favorites and recent user files | [Guide](apps/yyclip/README.md) |
| **yyfilemanager** | Folder synchronization, search, duplicate/version cleanup and recovery | [Guide](crates/yy-win/help/filemanager.md) |
| **yybrowser** | Tabbed browsing, profiles, proxies and ad blocking | [Guide](crates/yy-win/help/browser.md) |
| **yydraw** | Architecture diagrams, cloud/business icons, grouping, alignment and SVG/PNG | [Guide](docs/user/yydraw.md) |
| **yydbmanager** | SQL for seven databases, execution plans, ER diagrams and import/export | [Guide](docs/user/yydbmanager.md) |
| **yymerge** | 2/3-way merge, folder sync, Excel/PowerPoint, image and PDF comparison | [Guide](docs/user/yymerge.md) |
| **YY3Designer** | Pixel art, image conversion, six-view 3D, thickness/rounding and PNG/OBJ | [Guide](docs/user/yy3designer.md) |
| **yy-agent** | Linux helper deployed to SSH hosts | [Guide](docs/developer/guide.ja.md) |

## Documentation

- **Users**: [Getting started, manuals and examples](docs/user/README.md), [feature guide](docs/user/applications.md).
- **Developers**: [Internal specifications, builds, tests and CI](docs/developer/README.md), [English developer guide](docs/developer/guide.md).
- **Decisions and progress**: [Proposals, reviews, validation and measurements](docs/history/README.md).

## Getting started

Extract the Windows x64 distribution and launch the application. yydbmanager, yymerge and YY3Designer have separate packages.
Open the bundled manual with F1 or the Help menu. Previews and browser features use WebView2.
yymerge also provides a Windows/Linux CLI. SSH and external databases require connection settings and appropriate host permissions.
Use each manual for supported formats and current limitations.

## Separate repositories

- [YYSort](https://github.com/ykwyuta/YYSort): Windows/Linux external sort for huge fixed-record/CSV files, a DFSORT control-card subset and CPU/memory limits.
- [YYLibrary](https://github.com/ykwyuta/YYLibrary): Shared encoding codecs and table generator, consumed through pinned Cargo Git revisions.
- [DuckDB-FileIndex](https://github.com/ykwyuta/DuckDB-FileIndex): Standalone DuckDB extension for persistent indexes on CSV and fixed-record files.

yysqlfile has been discontinued; file indexing is provided by DuckDB-FileIndex.

## License

[GPL-3.0-or-later](COPYING) with the [WebView2 Loader additional permission](COPYING.EXCEPTION).
Bundled assets and dependencies retain their own terms; see the [license guide](docs/user/applications.md#license).
