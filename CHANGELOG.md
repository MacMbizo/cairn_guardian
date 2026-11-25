# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Renamed the product from Aegis Guardian to **Cairn Guardian**. The app, tray, notifications, dialogs and executable (`cairn-guardian.exe`) all use the new name.
- New Backup drives use a `Cairn Backup` folder. An existing `Aegis Backup` folder is renamed automatically when the drive is next plugged in, and the catalog follows.
- New quarantine items go to `.cairn-quarantine`. An existing `.aegis-quarantine` stays in use until its items expire, so every undo keeps working.
- The old "Aegis Guardian" sign-in entry is replaced by Cairn's own.
- `.cairnignore` and `.cairnboundary` marker files are supported; the older `.aegis*` names still work.

## [0.1.0] - 2026-11-25

First complete build, with all fifteen planned capabilities:

- encrypted catalog with live NTFS change tracking across the PC and external drives;
- project, set and module recognition, with project plans that bring split projects together;
- filing rules, Folderless mode, and learn-then-take-over;
- hybrid keyword and semantic search, and *Ask Files* using local models;
- Backup drive mirror, duplicates across drives, space trends and a workspace health score;
- System care, Forgotten assets, Version tracking, Knowledge base, Research librarian and Cross-project insight;
- Self-healing links and tags, Data governance, the Chief Digital Officer view, and Digital estate.

[Unreleased]: https://github.com/MacMbizo/cairn_guardian/compare/master...HEAD
[0.1.0]: https://github.com/MacMbizo/cairn_guardian/tree/master
