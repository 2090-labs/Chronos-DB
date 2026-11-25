//! Soak of attached write-back and its recovery: for CHRONOS_SOAK_MINUTES, round after round, a world makes a few
//! random changes (updates, inserts, deletes) and a child process merges it, killed (abort, as kill -9) at a random
//! step of the write-back (`CHRONOS_ATTACH_CRASH`) or at a random file step of a checkpoint after it
//! (`CHRONOS_CRASH_AT`), or not at all. After each round the folder is opened again and the merge settled, and then:
//! nothing is left to settle, main holds exactly what the source holds, the merge is in the source's merge table
//! and main's ledger the same number of times (0 or 1), and the world is gone exactly when the source committed it.
//!
//! Each round's seed comes from the run's start seed and its number, and a round works on what the rounds before it
//! left, so a failure is replayed by running the same sequence again: a failure stops the soak and keeps the
//! folder, naming the start seed and the round; CHRONOS_SOAK_SEED=<start seed> starts a run from it, and
//! CHRONOS_SOAK_ROUNDS=<round> stops it after that round (the failing one). Without CHRONOS_SOAK_MINUTES every test
//! here returns at once. The tests set the process's CHRONOS_SOURCE_URL, so they take turns (`SOAK`). Sources: CHRONOS_TEST_PG (`soak_postgres`), CHRONOS_TEST_MYSQL (`soak_mysql`), CHRONOS_TEST_MONGO
//! (`soak_mongo`, built with `--features mongo`). `tools/attach-soak.sh` runs one with a log.

use chronos::sql::run;
use chronos::{Db, MAIN};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One soak at a time: each sets the process-wide CHRONOS_SOURCE_URL its merge children read.
static SOAK: Mutex<()> = Mutex::new(());

/// Whether a child process stopped at a crash point (`std::process::abort()`: SIGABRT).
fn aborted(s: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        s.signal() == Some(6)
    }
    #[cfg(not(unix))]
    {
        !s.success()
    }
}

/// xorshift64*: the same seed gives the same round.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The source database beside the folder: its rows (`id=v`, sorted) and how many merges it has recorded.
trait Source {
    fn name(&self) -> &'static str;
    fn rows(&mut self) -> Vec<String>;
    fn merges(&mut self) -> u64;
    /// The same rows as main has them (sorted).
    fn main_rows(&self, db: &Db) -> Vec<String>;
    /// SQL, in a world, for: set row `id` to `v`; insert a row (`id` free in main) with `v`; delete row `id`.
    fn update(&self, id: &str, v: &str) -> String;
    fn insert(&self, id: u64, v: &str) -> String;
    fn delete(&self, id: &str) -> String;
    /// The write-back steps a merge passes beside this source.
    fn steps(&self) -> &'static [&'static str] {
        &["before_commit", "after_commit", "after_publish", "before_forget"]
    }
}

/// Main's rows of `sql` (one text column), sorted.
fn main_list(db: &Db, sql: &str) -> Vec<String> {
    let mut v: Vec<String> = run(db, MAIN, sql, &[])
        .unwrap_or_else(|e| panic!("{sql}: {}", e.message))
        .pop()
        .unwrap()
        .rows
        .iter()
        .map(|r| r[0].to_text())
        .collect();
    v.sort();
    v
}

fn minutes() -> Option<f64> {
    std::env::var("CHRONOS_SOAK_MINUTES").ok().and_then(|m| m.parse().ok())
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("chronos-soak-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn attach(dir: &Path, table: &str, extra: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_chronos"))
        .arg("attach")
        .arg(dir)
        .args(["--tables", table])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "attach: {}", String::from_utf8_lossy(&out.stderr));
}

/// Settles what a killed merge left (waiting while the source can't tell yet, as MongoDB with a killed transaction
/// still open): what it said.
fn settle(db: &Db) -> Result<String, String> {
    let started = Instant::now();
    loop {
        match chronos::attach::resolve(db) {
            Ok(s) => return Ok(s.unwrap_or_default()),
            Err(e) if e.contains("can't tell") && started.elapsed() < Duration::from_secs(180) => {
                std::thread::sleep(Duration::from_secs(1))
            }
            Err(e) => return Err(e),
        }
    }
}

/// One round; Err says what broke.
fn round(src: &mut dyn Source, dir: &Path, n: u64, rng: &mut Rng) -> Result<String, String> {
    let w = format!("s{n}");
    let db = Db::open(dir).map_err(|e| format!("open: {e:?}"))?;
    let have = src.main_rows(&db);
    let ids: Vec<String> = have.iter().map(|r| r.split_once('=').unwrap().0.to_string()).collect();
    run(&db, MAIN, &format!("create world {w}"), &[]).map_err(|e| e.message)?;
    let mut ops = vec![];
    let mut touched = std::collections::BTreeSet::new();
    for _ in 0..1 + rng.below(4) {
        let v = format!("r{n}x{}", rng.below(1000));
        let pick = (!ids.is_empty()).then(|| ids[rng.below(ids.len() as u64) as usize].clone());
        let sql = match (rng.below(10), pick) {
            (0..=5, Some(id)) if touched.insert(id.clone()) => src.update(&id, &v),
            (6..=7, Some(id)) if ids.len() > 5 && touched.insert(id.clone()) => src.delete(&id),
            _ => {
                let id = 1000 + n * 10 + ops.len() as u64;
                src.insert(id, &v)
            }
        };
        run(&db, &w, &sql, &[]).map_err(|e| format!("{sql}: {}", e.message))?;
        ops.push(sql);
    }
    drop(db);
    let (merges0, ledger0) = {
        let db = Db::open(dir).map_err(|e| format!("open: {e:?}"))?;
        (src.merges(), db.ledger_rows().unwrap().len())
    };
    // where to crash: a write-back step (half the rounds), a checkpoint's file step after a clean merge (a quarter),
    // or nowhere
    let steps = src.steps();
    let kind = rng.below(4);
    let at = match kind {
        0 | 1 => format!("attach:{}", steps[rng.below(steps.len() as u64) as usize]),
        2 => format!("disk:{}", 1 + rng.below(8)),
        _ => "none".into(),
    };
    let mut merge = Command::new(env!("CARGO_BIN_EXE_chronos"));
    merge.args(["-b", &w, dir.to_str().unwrap(), "merge"]);
    if let Some(step) = at.strip_prefix("attach:") {
        merge.env("CHRONOS_ATTACH_CRASH", step);
    }
    let out = merge.output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if at.starts_with("attach:") {
        if !aborted(&out.status) {
            return Err(format!("{at}: the merge didn't stop at its crash point: {:?} {said}", out.status));
        }
    } else if !out.status.success() {
        return Err(format!("{at}: the merge failed: {said}"));
    }
    if let Some(k) = at.strip_prefix("disk:") {
        let out = Command::new(env!("CARGO_BIN_EXE_chronos"))
            .args([dir.to_str().unwrap(), "checkpoint"])
            .env("CHRONOS_CRASH_AT", k)
            .output()
            .unwrap();
        if !out.status.success() && !aborted(&out.status) {
            return Err(format!("{at}: checkpoint failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
    }
    let db = Db::open(dir).map_err(|e| format!("{at}: open after the crash: {e:?}"))?;
    let settled = settle(&db).map_err(|e| format!("{at}: settle: {e}"))?;
    if dir.join("attached.pending").exists() {
        return Err(format!("{at}: still pending after settling ({settled})"));
    }
    if let Some(again) = chronos::attach::resolve(&db).map_err(|e| format!("{at}: settle again: {e}"))? {
        return Err(format!("{at}: settled twice: {again}"));
    }
    let (here, there) = (src.main_rows(&db), src.rows());
    if here != there {
        return Err(format!(
            "{at}: main and {} differ ({settled})\n  ops: {ops:?}\n  main:   {here:?}\n  source: {there:?}",
            src.name()
        ));
    }
    let in_source = src.merges() - merges0;
    let in_ledger = (db.ledger_rows().unwrap().len() - ledger0) as u64;
    let gone = db.world(&w).is_err();
    if in_source > 1 || in_source != in_ledger || gone != (in_source == 1) {
        return Err(format!(
            "{at}: merges in {} {in_source}, in main's ledger {in_ledger}, world gone {gone} ({settled})",
            src.name()
        ));
    }
    if !gone {
        run(&db, MAIN, &format!("drop world {w}"), &[]).map_err(|e| e.message)?;
    }
    Ok(format!("{at} ops={} merged={gone} rows={}", ops.len(), here.len()))
}

/// Rounds until the time is up; panics at the first failure, keeping the folder.
fn soak(src: &mut dyn Source, dir: &Path, mins: f64) {
    let start_seed = std::env::var("CHRONOS_SOAK_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 | 1
    });
    let deadline = Instant::now() + Duration::from_secs_f64(mins * 60.0);
    let started = Instant::now();
    let (mut n, mut merged) = (0u64, 0u64);
    let mut by_point = std::collections::BTreeMap::<String, u64>::new();
    let last: Option<u64> = std::env::var("CHRONOS_SOAK_ROUNDS").ok().and_then(|s| s.parse().ok());
    while Instant::now() < deadline && last.is_none_or(|l| n < l) {
        n += 1;
        let seed = start_seed.wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1;
        let mut rng = Rng(seed);
        match round(src, dir, n, &mut rng) {
            Ok(line) => {
                println!("soak {} round {n} seed {seed}: {line}", src.name());
                merged += u64::from(line.contains("merged=true"));
                *by_point.entry(line.split(' ').next().unwrap().to_string()).or_default() += 1;
            }
            Err(e) => panic!(
                "soak {} round {n} seed {seed} FAILED after {:.1} min, folder kept at {}: {e}\nreplay: CHRONOS_SOAK_SEED={start_seed} CHRONOS_SOAK_ROUNDS={n}",
                src.name(),
                started.elapsed().as_secs_f64() / 60.0,
                dir.display()
            ),
        }
    }
    println!(
        "soak {} done: {n} rounds in {:.1} min, {merged} merged, 0 divergences, start seed {start_seed}; by crash point: {by_point:?}",
        src.name(),
        started.elapsed().as_secs_f64() / 60.0
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Postgres

struct Pg(postgres::Client);

fn pg_rows(c: &mut postgres::Client, sql: &str) -> Vec<String> {
    let mut v: Vec<String> = c
        .simple_query(sql)
        .unwrap()
        .iter()
        .filter_map(|m| match m {
            postgres::SimpleQueryMessage::Row(r) => Some(r.get(0).unwrap_or("NULL").to_string()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

impl Source for Pg {
    fn name(&self) -> &'static str {
        "postgres"
    }
    fn rows(&mut self) -> Vec<String> {
        pg_rows(&mut self.0, "select id || '=' || v from t")
    }
    fn merges(&mut self) -> u64 {
        pg_rows(&mut self.0, "select count(*) from chronos.merges where id like 'merge:%'")[0].parse().unwrap()
    }
    fn main_rows(&self, db: &Db) -> Vec<String> {
        main_list(db, "select id || '=' || v from t")
    }
    fn update(&self, id: &str, v: &str) -> String {
        format!("update t set v = '{v}' where id = {id}")
    }
    fn insert(&self, id: u64, v: &str) -> String {
        format!("insert into t values ({id}, '{v}')")
    }
    fn delete(&self, id: &str) -> String {
        format!("delete from t where id = {id}")
    }
}

#[test]
fn soak_postgres() {
    let (Some(mins), Ok(admin_url)) = (minutes(), std::env::var("CHRONOS_TEST_PG")) else {
        eprintln!("skipped: set CHRONOS_SOAK_MINUTES and CHRONOS_TEST_PG");
        return;
    };
    let _one = SOAK.lock().unwrap_or_else(|e| e.into_inner());
    let name = format!("chronos_soak_{}", std::process::id());
    let mut admin = postgres::Client::connect(&admin_url, postgres::NoTls).unwrap();
    // (one statement per call: two in one batch run as one transaction, where Postgres refuses DROP DATABASE)
    admin.batch_execute(&format!("drop database if exists {name}")).unwrap();
    admin.batch_execute(&format!("create database {name}")).unwrap();
    let url = format!("{}/{name}", admin_url.rsplit_once('/').expect("CHRONOS_TEST_PG names a database").0);
    unsafe { std::env::set_var("CHRONOS_SOURCE_URL", &url) };
    let mut pg = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    pg.batch_execute("create table t (id int primary key, v text not null); insert into t select g, 'a' from generate_series(1, 20) g")
        .unwrap();
    let dir = folder("pg");
    attach(&dir, "t", &[]);
    soak(&mut Pg(pg), &dir, mins);
    let _ = admin.batch_execute(&format!("drop database if exists {name} with (force)"));
}

// ---- MySQL

struct My(chronos::myclient::Client);

impl Source for My {
    fn name(&self) -> &'static str {
        "mysql"
    }
    fn rows(&mut self) -> Vec<String> {
        let done = self.0.query("select concat(id, '=', v) from t").unwrap();
        let mut v: Vec<String> =
            done.last().map(|d| d.rows.iter().map(|r| r[0].clone().unwrap_or_default()).collect()).unwrap_or_default();
        v.sort();
        v
    }
    fn merges(&mut self) -> u64 {
        let done = self.0.query("select count(*) from chronos_merges where id like 'merge:%'").unwrap();
        done.last().unwrap().rows[0][0].as_deref().unwrap().parse().unwrap()
    }
    fn main_rows(&self, db: &Db) -> Vec<String> {
        main_list(db, "select id || '=' || v from t")
    }
    fn update(&self, id: &str, v: &str) -> String {
        format!("update t set v = '{v}' where id = {id}")
    }
    fn insert(&self, id: u64, v: &str) -> String {
        format!("insert into t values ({id}, '{v}')")
    }
    fn delete(&self, id: &str) -> String {
        format!("delete from t where id = {id}")
    }
}

#[test]
fn soak_mysql() {
    use chronos::myclient::{self, Client};
    let (Some(mins), Ok(admin_url)) = (minutes(), std::env::var("CHRONOS_TEST_MYSQL")) else {
        eprintln!("skipped: set CHRONOS_SOAK_MINUTES and CHRONOS_TEST_MYSQL");
        return;
    };
    let _one = SOAK.lock().unwrap_or_else(|e| e.into_inner());
    let name = format!("chronos_soak_{}", std::process::id());
    let mut admin = Client::connect(&myclient::parse(&admin_url).unwrap()).unwrap();
    admin.query(&format!("drop database if exists {name}; create database {name}")).unwrap();
    let url = match admin_url.split_once('?') {
        Some((base, q)) => format!("{}/{name}?{q}", base.rsplit_once('/').unwrap().0),
        None => format!("{}/{name}", admin_url.rsplit_once('/').unwrap().0),
    };
    unsafe { std::env::set_var("CHRONOS_SOURCE_URL", &url) };
    let mut my = Client::connect(&myclient::parse(&url).unwrap()).unwrap();
    my.query("create table t (id int primary key, v varchar(40) not null) engine = InnoDB").unwrap();
    let values: Vec<String> = (1..=20).map(|i| format!("({i}, 'a')")).collect();
    my.query(&format!("insert into t values {}", values.join(", "))).unwrap();
    let dir = folder("mysql");
    attach(&dir, "t", &[]);
    soak(&mut My(my), &dir, mins);
    let _ = admin.query(&format!("drop database if exists {name}"));
}

// ---- MongoDB

#[cfg(feature = "mongo")]
struct Mongo(mongodb::sync::Database);

#[cfg(feature = "mongo")]
impl Source for Mongo {
    fn name(&self) -> &'static str {
        "mongodb"
    }
    fn rows(&mut self) -> Vec<String> {
        use mongodb::bson::{doc, Bson, Document};
        let mut v: Vec<String> = self
            .0
            .collection::<Document>("leads")
            .find(doc! {})
            .run()
            .unwrap()
            .map(|d| {
                let d = d.unwrap();
                let id = match d.get("_id").unwrap() {
                    Bson::Int32(i) => i.to_string(),
                    Bson::Int64(i) => i.to_string(),
                    Bson::String(s) => s.clone(),
                    Bson::ObjectId(o) => o.to_hex(),
                    other => format!("{other}"),
                };
                format!("{id}={}", d.get_str("v").unwrap_or("NULL"))
            })
            .collect();
        v.sort();
        v
    }
    fn merges(&mut self) -> u64 {
        use mongodb::bson::{doc, Document};
        self.0
            .collection::<Document>("chronos_merges")
            .count_documents(doc! { "_id": { "$regex": "^merge:" } })
            .run()
            .unwrap()
    }
    fn main_rows(&self, db: &Db) -> Vec<String> {
        main_list(db, "select _id || '=' || (doc->>'v') from leads")
    }
    fn update(&self, id: &str, v: &str) -> String {
        format!("update leads set doc = jsonb_set(doc, '{{v}}', '\"{v}\"') where _id = '{id}'")
    }
    fn insert(&self, _id: u64, v: &str) -> String {
        format!("insert into leads (org_id, doc) values (42, '{{\"v\": \"{v}\"}}')")
    }
    fn delete(&self, id: &str) -> String {
        format!("delete from leads where _id = '{id}'")
    }
}

#[cfg(feature = "mongo")]
#[test]
fn soak_mongo() {
    use mongodb::bson::{doc, Document};
    let (Some(mins), Ok(admin)) = (minutes(), std::env::var("CHRONOS_TEST_MONGO")) else {
        eprintln!("skipped: set CHRONOS_SOAK_MINUTES and CHRONOS_TEST_MONGO");
        return;
    };
    let _one = SOAK.lock().unwrap_or_else(|e| e.into_inner());
    let name = format!("chronos_soak_{}", std::process::id());
    let (base, query) = admin.split_once('?').unwrap_or((admin.as_str(), ""));
    let base = base.trim_end_matches('/');
    let base = match base.split_once("://").map(|(_, rest)| rest.contains('/')) {
        Some(true) => base.rsplit_once('/').unwrap().0,
        _ => base,
    };
    let url = format!("{base}/{name}{}{query}", if query.is_empty() { "" } else { "?" });
    unsafe { std::env::set_var("CHRONOS_SOURCE_URL", &url) };
    let client = mongodb::sync::Client::with_uri_str(&url).unwrap();
    let mdb = client.database(&name);
    mdb.drop().run().unwrap();
    mdb.collection::<Document>("leads")
        .insert_many((1..=20).map(|i| doc! { "_id": i, "org_id": 42, "v": "a" }))
        .run()
        .unwrap();
    let dir = folder("mongo");
    attach(&dir, "leads", &["--fields", "org_id"]);
    soak(&mut Mongo(mdb.clone()), &dir, mins);
    mdb.drop().run().unwrap();
}
