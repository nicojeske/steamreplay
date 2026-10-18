# M16: Name the clip before export

## 1. Goal and user-visible outcome
Before M16 a clip was always called `<Game>_<date>.mp4`, unless you rewrote the whole output path. Its Fireshare title was typed separately in the upload dialog. After M16 the Export tab has a **Name** field. The name is used three times:
- for the local file, `<folder>/<name>.mp4`,
- as the mp4 `title` tag,
- as the prefilled Fireshare title.

You name the clip once, and the file and the upload match.

## 2. Prerequisites
- M3/M5 (export), M9 (export queue, Export tab), M10 (Fireshare upload dialog and jobs).

## 3. Scope / non-goals
**In scope:**
- The Name field.
- A Folder field with a folder picker, which replaces the full-path field.
- The name as the default upload title, both for "Export & upload" and for a later upload of the export.

**Non-goals:**
- Renaming exports that already exist.
- Name templates (for example `{game} {date}`).
- Syncing a title changed in the upload dialog back to the file name.

## 4. Design

### Backend
- `ExportRequest.output: Option<PathBuf>` is replaced by `name: Option<String>` and `dir: Option<PathBuf>`.
  - `dir: None` uses the account's export folder (`export_dir`: the settings override, else Steam's `ExportDirectory`, else the system video folder).
  - A blank or missing `name` gives `default_file_name` (`<Game>_<YYYY-MM-DD_HH-MM-SS>.mp4`), as before.
  - The path is resolved when the job starts and run through `dedupe` (`_2`, `_3`, …), so queued jobs with the same name do not collide.
- `export::clip_file_name(name)` turns a name into a file name:
  - It strips a typed `.mp4` suffix (any case).
  - It removes the characters Windows forbids (`sanitize_name`).
  - It cuts the name to 120 chars.
  - It appends `_` to Windows device names (`CON`, `NUL`, `COM1`, …).
  - It keeps spaces and Unicode.
- The mp4 `title` tag is the name when one is given, else `"<Game> <started_at>"` as before.
- `RecentExport.name: Option<String>` (`serde(default)`, so older stored lists still load) remembers the name for later uploads.
- `default_export_dir(id)` is a new command that gives the folder the Folder field shows. `default_export_path` stays: the file stem it returns is the Name placeholder.

### Frontend
- **Export tab, Output section:**
  - **Name:** empty by default, with the default file stem as its placeholder; at most 120 chars. Enter exports.
  - **Folder:** the default export folder, editable, with a folder picker.
  - After an export is queued, the name is cleared and the folder is kept.
- **Job:** `ExportSpec` carries `name` and `dir`, and the job's `defaultTitle` is the name, or `Deadlock – 17 Oct 2026, 21:03` without one. That value already feeds the upload dialog, the export-done toast and the job rows.
- **Recent exports menu:** "Upload to Fireshare" suggests `entry.name` when it is set.
- **Upload dialog:** unchanged. Its title is still editable.

## 5. Edge cases
- **The name already exists in the folder:** the file becomes `Name_2.mp4`. The toast shows the real name.
- **A name that is only forbidden characters:** the file becomes `Clip.mp4`.
- **The folder does not exist:** it is created before the export.
- **The title is cleared in the upload dialog:** Fireshare falls back to the file name, which is now the clip name anyway.

## 6. Acceptance
- An empty name gives the same file and title as before M16.
- `Ace on Inferno` gives `<export folder>/Ace on Inferno.mp4`, whose mp4 title tag is `Ace on Inferno`. The upload dialog's title is prefilled with `Ace on Inferno`.
- The same name again gives `Ace on Inferno_2.mp4`.
- A folder chosen with the picker is where the file is written.
- "Upload to Fireshare" in the recent exports menu suggests the name.

## 7. Verification
- `cargo test`: the `clip_file_name` unit tests.
- `pnpm typecheck`, `pnpm lint`.
- Manual: the acceptance list in the running app, and `ffprobe` for the title tag.

## 8. Open questions
- None.
