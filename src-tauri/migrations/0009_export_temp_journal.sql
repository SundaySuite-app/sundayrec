-- SundayRec migration 0009 — the export's render-temp journal (F2-4b)
--
-- Every editor export renders into `<name>.__editor_tmp.<ext>` beside its
-- destination and renames it onto the delivered name once ffmpeg exits zero
-- (F2-4). An abort takes the temp with it (`TempRender`'s Drop), and the
-- startup sweep reaps what a hard crash leaves behind — but the sweep only
-- knew to look in the save folder and the library's folders. A folder picked
-- by hand for one export, or the folder of a file opened from outside the
-- library, was never looked in again, so a power cut mid-export could leave a
-- full-size half-written file there for good.
--
-- A row here is written BEFORE ffmpeg starts writing the temp and deleted once
-- the temp has been renamed or removed. A row that survives to the next launch
-- is therefore exactly a render that never finished, and names the one file
-- the sweep may delete (`src-tauri/src/editor/export_journal.rs`, which also
-- re-checks the path's shape and the file's type before it deletes anything).
--
-- Local only: the path is a path on this machine, like `recording.file_path`,
-- and nothing reads this table but the export and the startup sweep. It is
-- normally empty — one row while an export runs, none otherwise.
--
-- `id` (UUID v7) and not `path` is the key: the temp name is deterministic, so
-- a crashed render and the next export of the same recording share a path, and
-- each must be able to drop ITS row without touching the other's.
create table if not exists export_temp (
  id         TEXT PRIMARY KEY NOT NULL,
  path       TEXT NOT NULL,              -- the temp, exactly as `editor_tmp_path` built it
  created_at INTEGER NOT NULL            -- unix ms, when the export journalled it
);

-- The sweep takes the oldest rows first, a bounded number per launch.
create index if not exists idx_export_temp_age on export_temp (created_at);
