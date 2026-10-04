//! Hvor appens egne data bor, og flyttingen fra Roaming til Local AppData på
//! Windows (eierbeslutning F-W10, avgjort 2026-10-04).
//!
//! ## Hvorfor
//!
//! På domenepåloggede Windows-maskiner synkroniseres Roaming-profilen over
//! nettverket ved inn- og utlogging. `sundayrec.sqlite` med `-wal` og `-shm`
//! er en stadig voksende fil som skrives hele tiden mens en tjeneste går: det
//! er nøyaktig den typen data Roaming håndterer dårlig (låste filer, sakte
//! utlogging, en profil-synk som tar en halv database midt i en skriving).
//! Loggene og pre-roll-tmp-mappa flyttet til Local i F-W6 (`util::
//! move_once_best_effort`). Dette er resten: databasen og alt som ligger ved
//! siden av den.
//!
//! ## Hva som lå i appdata (Windows: `%APPDATA%\no.sundayrec.app`)
//!
//! | Sti                                   | Skjebne                                  |
//! |---------------------------------------|------------------------------------------|
//! | `sundayrec.sqlite` (+ `-wal`, `-shm`) | KOPIERES til Local (denne modulen)       |
//! | `recovery/<session>.json`             | nye skrives til Local; gamle SKANNES i Roaming |
//! | `last-recording.json`, `recording-telemetry-history.json`, `last-error.json`, `crashes/` | kopieres best-effort |
//! | `update-relaunch.log`, `SundayRec-diagnose.md` | skrives i Local fra nå; ikke kopiert (flyktige) |
//! | `logs/`, `tmp/`                       | flyttet allerede i F-W6                  |
//!
//! Papirkurven (`.sundayrec-trash/`) og capture-mappene (`.sundayrec-capture-*`)
//! ligger i OPPTAKSMAPPA, ikke i appdata, og berøres ikke. Opptaksmappa er
//! brukerdata og flyttes aldri.
//!
//! ## Flyttingen (kun Windows, én gang, FØR `open_pool`)
//!
//! 1. Har Local en database (>0 byte), er den sannheten. Ferdig.
//! 2. Har ikke Roaming noen: ny installasjon. Local brukes.
//! 3. Ellers, [`move_database`]: åpne den gamle, `PRAGMA wal_checkpoint(TRUNCATE)`
//!    (alt i hovedfila, og `busy` må være 0), tell rader i `recording` og
//!    `app_setting`, kopier til `sundayrec.sqlite.flytter` i Local, `fsync`,
//!    verifiser kopien (`PRAGMA integrity_check` + samme radtall), og gi den så
//!    navnet `sundayrec.sqlite` med én atomisk `rename`. En avbrutt flytting
//!    etterlater bare `.flytter`-fila, som neste oppstart skriver over.
//! 4. Feiler noe av det: [`Outcome::FellBack`]. Appen bruker Roaming for denne
//!    økta (akkurat som før), logger, og varsler én gang. Den starter ALDRI med
//!    en tom database når Roaming har data.
//!
//! ## Den gamle fila: LA DEN STÅ, urørt
//!
//! Roaming-databasen slettes ikke og får ikke nytt navn. Begrunnelse: v0.25.0
//! og eldre leter etter `sundayrec.sqlite` i Roaming. Gjør en frivillig en
//! nedgradering (en feilet oppdatering, en gammel installer fra et USB-minne),
//! åpner den gamle appen ellers en TOM database på et tomt sted og viser
//! historikk og innstillinger som borte, midt i en tjeneste. Står den gamle
//! fila der, åpner den en FORELDET men ekte kopi: sist ukes historikk i stedet
//! for ingenting. Det er det tryggeste feilutfallet vi kan velge. Prisen er at
//! en nedgradert økt skriver til den gamle fila, og at neste oppgradering
//! ikke henter det tilbake (Local finnes, og vinner). Det er en bevisst
//! avveining: å flette to databaser uten å vite hvilken som er riktig ville
//! vært verre enn å miste en ukes innstillingsendringer. Den kostbare
//! restfila er også kjent og avgrenset: den vokser ikke lenger, så Roaming-
//! synkroniseringen tar den bare én gang.
//!
//! (Den ene lille endringen i Roaming er selve sjekkpunktet: `wal_checkpoint`
//! skriver innholdet fra `-wal` inn i hovedfila og tømmer `-wal`. Det er
//! datamessig en no-op for den som leser fila etterpå, og er nettopp det som
//! gjør at en nedgradert app ser alt.)
//!
//! ## Recovery
//!
//! Et opptak som krasjet like før oppdateringen har manifestet sitt i
//! `Roaming\…\recovery`. Første oppstart etter oppdateringen MÅ finne det, så
//! recovery-skanningen leser BEGGE steder ([`Choice::dirs_to_scan`]); nye
//! manifester skrives der databasen bor. Skanningen sletter manifestet der det
//! ble funnet.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row, SqliteConnection};

/// Databasefilas navn i appdata-mappa. Samme navn begge steder.
pub const DB_FILE: &str = "sundayrec.sqlite";

/// Den halvferdige kopien i Local. Aldri `sundayrec.sqlite` før den er
/// verifisert, så en avbrutt flytting kan ikke se ut som en ferdig database.
const TEMP_FILE: &str = "sundayrec.sqlite.flytter";

/// Småfilene som følger med ved en vellykket flytting (best-effort, kopi).
const STATE_FILES: &[&str] = &[
    "last-recording.json",
    "recording-telemetry-history.json",
    "last-error.json",
];

/// Tabellene vi teller før og etter kopieringen. Historikken og innstillingene:
/// de to tingene en frivillig ville savnet.
const COUNTED_TABLES: &[(&str, &str)] = &[
    ("recording", "SELECT count(*) FROM recording"),
    ("app_setting", "SELECT count(*) FROM app_setting"),
];

/// Hva [`resolve`] kom frem til.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Ikke Windows (eller Roaming og Local er samme mappe): ingenting rørt,
    /// stien er byte-for-byte den den alltid var.
    Unchanged,
    /// Local hadde allerede databasen (andre oppstart og senere).
    AlreadyLocal,
    /// Ingen database noe sted: ny installasjon, Local brukes.
    Fresh,
    /// Databasen ble flyttet denne gangen.
    Moved { recordings: i64, settings: i64 },
    /// Flyttingen feilet; Roaming brukes i denne økta.
    FellBack { reason: String },
}

/// Mappa appen bruker, og den andre den fortsatt må lete i.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Der databasen og alt nytt skrives.
    pub active: PathBuf,
    /// Den andre plasseringen. Recovery-skanningen leser den også.
    pub other: Option<PathBuf>,
    pub outcome: Outcome,
}

impl Choice {
    /// `<active>/<sub>` og så `<other>/<sub>`: alle steder en «uferdig»-fil
    /// kan ligge. Aktiv først, så nye manifester vinner ved samme navn.
    pub fn dirs_to_scan(&self, sub: &str) -> Vec<PathBuf> {
        let mut dirs = vec![self.active.join(sub)];
        if let Some(other) = &self.other {
            dirs.push(other.join(sub));
        }
        dirs
    }
}

static CHOICE: OnceLock<Choice> = OnceLock::new();

/// Registrer valget (kalles én gang fra `setup`, før noe leser appdata).
pub fn install(choice: Choice) {
    let _ = CHOICE.set(choice);
}

/// Appens datamappe: den valgte, ellers Tauris egen (tester og tidlig oppstart).
pub fn dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, tauri::Error> {
    use tauri::Manager;
    match CHOICE.get() {
        Some(c) => Ok(c.active.clone()),
        None => app.path().app_data_dir(),
    }
}

/// [`Choice::dirs_to_scan`] for den installerte `Choice`en.
pub fn scan_dirs<R: tauri::Runtime>(app: &tauri::AppHandle<R>, sub: &str) -> Vec<PathBuf> {
    use tauri::Manager;
    match CHOICE.get() {
        Some(c) => c.dirs_to_scan(sub),
        None => app
            .path()
            .app_data_dir()
            .map(|d| vec![d.join(sub)])
            .unwrap_or_default(),
    }
}

fn non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Avgjør hvor appdata bor, og flytter databasen om det trengs. Ingen
/// filsystem-tilgang i det hele tatt når `windows` er `false`.
pub async fn resolve(roaming: &Path, local: &Path, windows: bool) -> Choice {
    if !windows || roaming == local {
        return Choice {
            active: roaming.to_path_buf(),
            other: None,
            outcome: Outcome::Unchanged,
        };
    }
    let roaming_db = roaming.join(DB_FILE);
    let local_db = local.join(DB_FILE);

    let to_local = |outcome| Choice {
        active: local.to_path_buf(),
        other: Some(roaming.to_path_buf()),
        outcome,
    };

    if non_empty_file(&local_db) {
        if non_empty_file(&roaming_db) {
            tracing::info!(
                "F-W10: the database exists in both Roaming and Local; Local is used, the Roaming file is left untouched"
            );
        }
        return to_local(Outcome::AlreadyLocal);
    }
    if !non_empty_file(&roaming_db) {
        return to_local(Outcome::Fresh);
    }

    match move_database(&roaming_db, &local_db).await {
        Ok((recordings, settings)) => {
            copy_state_files(roaming, local);
            tracing::info!(
                recordings,
                settings,
                "F-W10: the database was moved from Roaming to Local AppData (the Roaming file is left untouched)"
            );
            to_local(Outcome::Moved {
                recordings,
                settings,
            })
        }
        Err(reason) => {
            tracing::error!(
                "F-W10: moving the database to Local AppData failed; this session runs on Roaming: {reason}"
            );
            let _ = std::fs::remove_file(local.join(TEMP_FILE));
            Choice {
                active: roaming.to_path_buf(),
                other: Some(local.to_path_buf()),
                outcome: Outcome::FellBack { reason },
            }
        }
    }
}

async fn connect(path: &Path, immutable: bool) -> Result<SqliteConnection, String> {
    let mut opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .busy_timeout(std::time::Duration::from_secs(30));
    if immutable {
        opts = opts.read_only(true).immutable(true);
    }
    opts.connect()
        .await
        .map_err(|e| format!("could not open {}: {e}", file_name(path)))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Radtall per telt tabell; `None` for en tabell som ikke finnes (en database
/// som aldri nådde migrasjonene teller likt på begge sider).
async fn count_rows(conn: &mut SqliteConnection) -> Result<Vec<Option<i64>>, String> {
    let mut out = Vec::new();
    for (table, count_sql) in COUNTED_TABLES {
        let exists: i64 =
            sqlx::query("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1")
                .bind(table)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| format!("could not read the schema: {e}"))?
                .get(0);
        if exists == 0 {
            out.push(None);
            continue;
        }
        let n: i64 = sqlx::query(*count_sql)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| format!("could not count {table}: {e}"))?
            .get(0);
        out.push(Some(n));
    }
    Ok(out)
}

/// Kopier `roaming_db` til `local_db` slik at `local_db` aldri er noe annet
/// enn en komplett, verifisert database. Returnerer `(recording, app_setting)`.
pub(crate) async fn move_database(
    roaming_db: &Path,
    local_db: &Path,
) -> Result<(i64, i64), String> {
    // 1. Alt fra -wal inn i hovedfila. TRUNCATE (ikke PASSIVE) venter på
    //    lesere og tømmer -wal; `busy != 0` betyr at det IKKE ble ferdig, og
    //    da er hovedfila ikke hele databasen.
    let source_counts = {
        let mut src = connect(roaming_db, false).await?;
        let row = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut src)
            .await
            .map_err(|e| format!("wal_checkpoint failed: {e}"))?;
        let busy: i64 = row.get(0);
        if busy != 0 {
            return Err("wal_checkpoint did not complete (database in use)".into());
        }
        let counts = count_rows(&mut src).await?;
        let _ = src.close().await;
        counts
    };

    // 2. Kopi til en tempfil i Local, med fsync.
    let dir = local_db
        .parent()
        .ok_or("the Local path has no parent folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create the Local folder: {e}"))?;
    let tmp = dir.join(TEMP_FILE);
    let _ = std::fs::remove_file(&tmp);
    // Local har ingen database (vi er her fordi den manglet eller var tom), så
    // en forlatt -wal/-shm der hører ikke til noe: bort, så de ikke kan
    // spilles av mot den nye fila.
    for side in ["-wal", "-shm"] {
        let mut name = local_db.as_os_str().to_owned();
        name.push(side);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
    copy_and_sync(roaming_db, &tmp).map_err(|e| format!("copy failed: {e}"))?;

    // 3. Verifiser kopien. `immutable` leser bare hovedfila og lager ingen
    //    -wal/-shm ved siden av den.
    let verified = async {
        let mut copy = connect(&tmp, true).await?;
        let check: Vec<String> = sqlx::query("PRAGMA integrity_check")
            .fetch_all(&mut copy)
            .await
            .map_err(|e| format!("integrity_check failed: {e}"))?
            .iter()
            .map(|r| r.get::<String, _>(0))
            .collect();
        if check != ["ok"] {
            return Err(format!(
                "integrity_check on the copy: {}",
                check.join("; ").chars().take(200).collect::<String>()
            ));
        }
        let counts = count_rows(&mut copy).await?;
        let _ = copy.close().await;
        if counts != source_counts {
            return Err(format!(
                "row counts differ: {source_counts:?} in Roaming, {counts:?} in the copy"
            ));
        }
        Ok(counts)
    }
    .await;
    let counts = match verified {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };

    // 4. Atomisk til sitt rette navn.
    std::fs::rename(&tmp, local_db).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename failed: {e}")
    })?;
    sync_dir(dir);
    Ok((counts[0].unwrap_or(0), counts[1].unwrap_or(0)))
}

fn copy_and_sync(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::File::create(to)?;
    std::io::copy(&mut src, &mut dst)?;
    dst.sync_all()
}

/// fsync av mappa, så selve rename-en overlever strømbrudd. Unix; Windows kan
/// ikke åpne en mappe som fil og har ingen tilsvarende.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Kopier småfilene og krasjringen som følger med flyttingen. Aldri over en
/// fil som allerede finnes, aldri feil.
fn copy_state_files(roaming: &Path, local: &Path) {
    for name in STATE_FILES {
        let (from, to) = (roaming.join(name), local.join(name));
        if from.is_file() && !to.exists() {
            let _ = std::fs::copy(&from, &to);
        }
    }
    let (from, to) = (roaming.join("crashes"), local.join("crashes"));
    if let Ok(entries) = std::fs::read_dir(&from) {
        let _ = std::fs::create_dir_all(&to);
        for e in entries.flatten() {
            let target = to.join(e.file_name());
            if e.path().is_file() && !target.exists() {
                let _ = std::fs::copy(e.path(), target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store;

    /// Roaming og Local som to tempmapper — sømmen for Windows-stiene.
    struct Seam {
        _root: tempfile::TempDir,
        roaming: PathBuf,
        local: PathBuf,
    }

    fn seam() -> Seam {
        let root = tempfile::tempdir().unwrap();
        let roaming = root.path().join("Roaming/no.sundayrec.app");
        let local = root.path().join("Local/no.sundayrec.app");
        std::fs::create_dir_all(&roaming).unwrap();
        Seam {
            _root: root,
            roaming,
            local,
        }
    }

    /// En ekte database i `dir`, med `n` opptak og to innstillinger. Poolen
    /// returneres levende: så lenge den lever, ligger de siste skrivingene i
    /// `-wal` og er IKKE checkpointet, nøyaktig som etter et krasj.
    async fn database_with(dir: &Path, n: usize) -> sqlx::SqlitePool {
        let pool = store::open_pool(&dir.join(DB_FILE)).await.unwrap();
        store::set_setting(&pool, "language", "\"no\"")
            .await
            .unwrap();
        store::set_setting(&pool, "churchName", "\"Oasen\"")
            .await
            .unwrap();
        for i in 0..n {
            sqlx::query(
                "INSERT INTO recording (id, file_path, started_at, duration_ms, created_at) \
                 VALUES (?1, ?2, ?3, 1000, ?3)",
            )
            .bind(format!("id-{i}"))
            .bind(format!("/x/{i}.mp3"))
            .bind(1_700_000_000_000.0 + i as f64)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    async fn count_in(path: &Path, table: &str) -> i64 {
        let mut c = connect(path, false).await.unwrap();
        let (_, sql) = COUNTED_TABLES.iter().find(|(t, _)| *t == table).unwrap();
        let n: i64 = sqlx::query(*sql).fetch_one(&mut c).await.unwrap().get(0);
        n
    }

    fn wal_len(dir: &Path) -> u64 {
        std::fs::metadata(dir.join("sundayrec.sqlite-wal"))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn en_flytting_tar_med_skrivinger_som_bare_la_i_wal() {
        let s = seam();
        let _live = database_with(&s.roaming, 40).await;
        assert!(wal_len(&s.roaming) > 0, "premisset: -wal har innhold");
        // Premisset, hardere: hovedfila ALENE mangler radene.
        let alone = tempfile::tempdir().unwrap();
        std::fs::copy(s.roaming.join(DB_FILE), alone.path().join(DB_FILE)).unwrap();
        let mut c = connect(&alone.path().join(DB_FILE), true).await.unwrap();
        let in_main: Option<i64> = sqlx::query("SELECT count(*) FROM recording")
            .fetch_one(&mut c)
            .await
            .ok()
            .map(|r| r.get(0));
        assert_ne!(in_main, Some(40), "premisset: hovedfila er ikke komplett");

        let c = resolve(&s.roaming, &s.local, true).await;

        assert_eq!(
            c.outcome,
            Outcome::Moved {
                recordings: 40,
                settings: 2
            }
        );
        assert_eq!(c.active, s.local);
        assert_eq!(count_in(&s.local.join(DB_FILE), "recording").await, 40);
        assert_eq!(count_in(&s.local.join(DB_FILE), "app_setting").await, 2);
        assert!(
            !s.local.join(TEMP_FILE).exists(),
            "ingen tempfil blir liggende"
        );
    }

    #[tokio::test]
    async fn flyttingen_sletter_ikke_roaming_og_den_gamle_fila_har_alt() {
        let s = seam();
        let _live = database_with(&s.roaming, 12).await;

        resolve(&s.roaming, &s.local, true).await;

        let old = s.roaming.join(DB_FILE);
        assert!(
            old.exists(),
            "Roaming-databasen skal stå der for en nedgradering"
        );
        // En nedgradert app (leter i Roaming) ser hele historikken, ikke en tom.
        assert_eq!(count_in(&old, "recording").await, 12);
        assert_eq!(count_in(&old, "app_setting").await, 2);
    }

    #[tokio::test]
    async fn en_feil_i_flyttingen_gir_fallback_til_roaming_uten_tap() {
        let s = seam();
        let pool = database_with(&s.roaming, 300).await;
        store::checkpoint_and_close(&pool).await;
        // Ødelegg en indre side i hovedfila: åpnes fortsatt, men
        // integrity_check (eller en lesing) feiler.
        let db = s.roaming.join(DB_FILE);
        let mut bytes = std::fs::read(&db).unwrap();
        assert!(bytes.len() > 4096 * 3, "premisset: flere sider å skade");
        for b in &mut bytes[4096 * 2..4096 * 3] {
            *b = 0xA5;
        }
        std::fs::write(&db, &bytes).unwrap();
        let before = std::fs::read(&db).unwrap();

        let c = resolve(&s.roaming, &s.local, true).await;

        assert!(
            matches!(c.outcome, Outcome::FellBack { .. }),
            "{:?}",
            c.outcome
        );
        assert_eq!(c.active, s.roaming, "fallback til Roaming");
        assert!(
            !s.local.join(DB_FILE).exists(),
            "ingen halv eller tom database i Local"
        );
        assert!(!s.local.join(TEMP_FILE).exists());
        assert_eq!(std::fs::read(&db).unwrap(), before, "Roaming urørt");
    }

    #[tokio::test]
    async fn en_kopi_som_feiler_integrity_check_gir_fallback_selv_om_radtallene_stemmer() {
        let s = seam();
        let pool = database_with(&s.roaming, 50).await;
        store::checkpoint_and_close(&pool).await;
        // Fjern indeksen fra skjemaet UTEN å frigi sidene: radene er hele (så
        // radtallene stemmer), men indekssidene er foreldreløse, og bare
        // integrity_check ser det.
        let db = s.roaming.join(DB_FILE);
        let mut c = connect(&db, false).await.unwrap();
        sqlx::query("PRAGMA writable_schema = ON")
            .execute(&mut c)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM sqlite_master WHERE type = 'index' AND name = 'idx_recording_created_at'",
        )
        .execute(&mut c)
        .await
        .unwrap();
        c.close().await.unwrap();
        let before = std::fs::read(&db).unwrap();

        let c = resolve(&s.roaming, &s.local, true).await;

        match &c.outcome {
            Outcome::FellBack { reason } => {
                assert!(reason.contains("integrity_check"), "{reason}")
            }
            other => panic!("forventet fallback, fikk {other:?}"),
        }
        assert_eq!(c.active, s.roaming);
        assert!(!s.local.join(DB_FILE).exists());
        assert_eq!(std::fs::read(&db).unwrap(), before, "Roaming urørt");
    }

    #[tokio::test]
    async fn en_local_mappe_som_ikke_kan_lages_gir_fallback_uten_tap() {
        let s = seam();
        let pool = database_with(&s.roaming, 5).await;
        store::checkpoint_and_close(&pool).await;
        // Local er en FIL: create_dir_all feiler.
        let blocked = s._root.path().join("Local-er-en-fil");
        std::fs::write(&blocked, b"x").unwrap();
        let local = blocked.join("no.sundayrec.app");

        let c = resolve(&s.roaming, &local, true).await;

        assert!(matches!(c.outcome, Outcome::FellBack { .. }));
        assert_eq!(c.active, s.roaming);
        assert_eq!(count_in(&s.roaming.join(DB_FILE), "recording").await, 5);
    }

    #[tokio::test]
    async fn andre_oppstart_flytter_ikke_igjen() {
        let s = seam();
        let pool = database_with(&s.roaming, 7).await;
        store::checkpoint_and_close(&pool).await;
        let first = resolve(&s.roaming, &s.local, true).await;
        assert!(matches!(first.outcome, Outcome::Moved { .. }));

        // Appen kjører i Local og skriver mer.
        let local_pool = store::open_pool(&s.local.join(DB_FILE)).await.unwrap();
        store::set_setting(&local_pool, "etterpaa", "\"ja\"")
            .await
            .unwrap();
        store::checkpoint_and_close(&local_pool).await;
        let local_bytes = std::fs::read(s.local.join(DB_FILE)).unwrap();

        let second = resolve(&s.roaming, &s.local, true).await;

        assert_eq!(second.outcome, Outcome::AlreadyLocal);
        assert_eq!(
            std::fs::read(s.local.join(DB_FILE)).unwrap(),
            local_bytes,
            "Local skrives ikke over av Roaming-fila"
        );
        assert_eq!(count_in(&s.local.join(DB_FILE), "app_setting").await, 3);
    }

    #[tokio::test]
    async fn en_tom_local_fil_regnes_ikke_som_en_database() {
        let s = seam();
        let pool = database_with(&s.roaming, 3).await;
        store::checkpoint_and_close(&pool).await;
        std::fs::create_dir_all(&s.local).unwrap();
        std::fs::write(s.local.join(DB_FILE), b"").unwrap();

        let c = resolve(&s.roaming, &s.local, true).await;

        assert!(matches!(c.outcome, Outcome::Moved { recordings: 3, .. }));
        assert_eq!(count_in(&s.local.join(DB_FILE), "recording").await, 3);
    }

    #[tokio::test]
    async fn en_ny_installasjon_uten_gammel_database_bruker_local() {
        let s = seam();
        let c = resolve(&s.roaming, &s.local, true).await;
        assert_eq!(c.outcome, Outcome::Fresh);
        assert_eq!(c.active, s.local);
        assert!(!s.roaming.join(DB_FILE).exists());
    }

    #[tokio::test]
    async fn en_forlatt_wal_ved_siden_av_en_manglende_local_database_spilles_ikke_av() {
        let s = seam();
        let pool = database_with(&s.roaming, 4).await;
        store::checkpoint_and_close(&pool).await;
        std::fs::create_dir_all(&s.local).unwrap();
        std::fs::write(s.local.join("sundayrec.sqlite-wal"), vec![7u8; 5000]).unwrap();
        std::fs::write(s.local.join("sundayrec.sqlite-shm"), vec![7u8; 32768]).unwrap();

        let c = resolve(&s.roaming, &s.local, true).await;

        assert!(matches!(c.outcome, Outcome::Moved { recordings: 4, .. }));
        assert_eq!(count_in(&s.local.join(DB_FILE), "recording").await, 4);
    }

    #[tokio::test]
    async fn smafilene_og_krasjringen_kopieres_med_men_roaming_beholder_sine() {
        let s = seam();
        let pool = database_with(&s.roaming, 1).await;
        store::checkpoint_and_close(&pool).await;
        std::fs::write(s.roaming.join("last-recording.json"), b"{\"a\":1}").unwrap();
        std::fs::create_dir_all(s.roaming.join("crashes")).unwrap();
        std::fs::write(s.roaming.join("crashes/crash-1-0.json"), b"{}").unwrap();

        resolve(&s.roaming, &s.local, true).await;

        assert!(s.local.join("last-recording.json").is_file());
        assert!(s.local.join("crashes/crash-1-0.json").is_file());
        assert!(s.roaming.join("last-recording.json").is_file());
        assert!(s.roaming.join("crashes/crash-1-0.json").is_file());
    }

    // ── Mac/Linux: ingenting endres ─────────────────────────────────────────

    #[tokio::test]
    async fn utenfor_windows_er_stien_uendret_og_ingenting_leses_eller_lages() {
        let s = seam();
        let pool = database_with(&s.roaming, 2).await;
        store::checkpoint_and_close(&pool).await;
        let before = std::fs::read(s.roaming.join(DB_FILE)).unwrap();

        let c = resolve(&s.roaming, &s.local, false).await;

        assert_eq!(c.active, s.roaming);
        assert_eq!(c.other, None);
        assert_eq!(c.outcome, Outcome::Unchanged);
        assert!(!s.local.exists(), "Local-mappa lages ikke");
        assert_eq!(std::fs::read(s.roaming.join(DB_FILE)).unwrap(), before);
    }

    /// Golden: på denne verten er appdata-stien den Tauri alltid har gitt, og
    /// `resolve` med vertens egen plattformsflagg returnerer den uendret.
    /// (Windows har egen test i `util`: Local ≠ Roaming.)
    #[tokio::test]
    #[cfg(not(windows))]
    async fn mac_og_linux_har_samme_appdata_sti_som_for() {
        let Some(roaming) = crate::util::app_data_dir() else {
            return;
        };
        assert_eq!(crate::util::app_local_data_dir(), Some(roaming.clone()));
        let c = resolve(&roaming, &roaming, cfg!(windows)).await;
        assert_eq!(c.active, roaming);
        assert_eq!(c.outcome, Outcome::Unchanged);
        #[cfg(target_os = "macos")]
        assert!(roaming.ends_with("Library/Application Support/no.sundayrec.app"));
    }

    // ── Recovery ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn skanningen_leser_begge_steder_etter_flyttingen() {
        let s = seam();
        let pool = database_with(&s.roaming, 1).await;
        store::checkpoint_and_close(&pool).await;
        std::fs::create_dir_all(s.roaming.join("recovery")).unwrap();
        std::fs::write(s.roaming.join("recovery/1700000000000.json"), b"{}").unwrap();

        let c = resolve(&s.roaming, &s.local, true).await;
        let dirs = c.dirs_to_scan("recovery");

        assert_eq!(
            dirs,
            vec![s.local.join("recovery"), s.roaming.join("recovery")]
        );
        assert!(dirs.iter().any(|d| d.join("1700000000000.json").exists()));
    }

    #[tokio::test]
    async fn etter_fallback_leses_begge_steder() {
        let s = seam();
        let c = Choice {
            active: s.roaming.clone(),
            other: Some(s.local.clone()),
            outcome: Outcome::FellBack { reason: "x".into() },
        };
        assert_eq!(c.dirs_to_scan("recovery").len(), 2);
    }
}
