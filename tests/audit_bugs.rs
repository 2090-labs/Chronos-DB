//! Regression tests for bugs found by the 2026-09-24 capability audit.
use chronos::{Db, MAIN, sql};

fn run(db: &Db, q: &str) -> String {
    format!("{:?}", sql::run(db, MAIN, q, &[]).unwrap())
}

#[test]
fn aggregate_inside_an_operator_types_without_panicking() {
    let db = Db::new();
    run(&db, "create table t (id int primary key, v text)");
    run(&db, "insert into t values (1, 'a'), (2, 'b')");
    let out = run(&db, "select count(*) || 'x' from t");
    assert!(out.contains("2x"), "{out}");
}

#[test]
fn nearest_to_null_on_an_indexed_vector_column_is_not_a_crash() {
    let db = Db::new();
    run(&db, "create table emb (id int primary key, e vector(3))");
    run(&db, "insert into emb select i, ('[' || i || ',1,2]')::vector from generate_series(1, 50) i");
    run(&db, "create index on emb using hnsw (e vector_l2_ops)");
    // Postgres: every distance is NULL, so any 3 rows come back
    let rows = sql::run(&db, MAIN, "select id from emb order by e <-> null limit 3", &[]).unwrap();
    assert!(format!("{rows:?}").matches("Int(").count() >= 3, "{rows:?}");
    let p = sql::run(&db, MAIN, "select id from emb order by e <-> $1 limit 3", &[chronos::sql::Value::Null]);
    assert!(p.is_ok(), "{p:?}");
}

#[test]
fn merges_settled_by_columns_are_in_history() {
    let dir = std::env::temp_dir().join(format!("chronos-audit-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Db::open(&dir).unwrap();
    let q = |b: &str, s: &str| format!("{:?}", sql::run(&db, b, s, &[]).unwrap());
    q(MAIN, "create table h (id int primary key, a text, b text)");
    q(MAIN, "insert into h values (1, 'x', 'x')");
    q(MAIN, "create world hx");
    q("hx", "update h set a = 'hx' where id = 1");
    q(MAIN, "update h set b = 'main' where id = 1");
    q(MAIN, "merge world hx by columns");
    let h = q(MAIN, "show history limit 10");
    assert!(h.contains("merged hx"), "{h}");
    drop(db);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn version_and_unknown_options_make_no_database_folder() {
    let dir = std::env::temp_dir().join(format!("chronos-audit-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let chronos =
        |a: &str| std::process::Command::new(env!("CARGO_BIN_EXE_chronos")).arg(a).current_dir(&dir).output().unwrap();
    let v = chronos("--version");
    assert!(v.status.success() && String::from_utf8_lossy(&v.stdout).starts_with("chronos "), "{v:?}");
    assert!(!chronos("--verbose").status.success());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no folder was created");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn integer_key_ranges_read_by_key_and_match_a_scan() {
    let db = Db::new();
    run(&db, "create table t (id bigint primary key, v int)");
    run(&db, "insert into t select i, i % 7 from generate_series(-50, 5000) i");
    let plan = run(&db, "explain select count(*) from t where id between 10 and 20");
    assert!(plan.contains("Index Scan using t_pkey"), "{plan}");
    for (cond, want) in [
        ("id between 10 and 20", 11),
        ("id >= 10 and id < 20", 10),
        ("id > 10 and id <= 20", 10),
        ("id between -5 and 5", 11),
        ("id between 4990 and 6000", 11),
        ("id between 20 and 10", 0),
        ("id between 1 and 200000", 5000), // wider than the lookup limit: a scan, same answer
        ("id > 4995 and id < 9223372036854775807", 5),
    ] {
        let out = run(&db, &format!("select count(*) from t where {cond}"));
        assert!(out.contains(&format!("Int({want})")), "{cond}: {out}");
    }
    let s = run(&db, "select sum(v) from t where id between 100 and 200 and v = 3");
    let scan = run(&db, "select sum(v) from t where id + 0 between 100 and 200 and v = 3");
    assert_eq!(s, scan);
}

#[test]
fn equality_on_a_big_table_scans_instead_of_indexing_it_all_in_memory() {
    let db = Db::new();
    run(&db, "create table small (id int primary key, v text)");
    run(&db, "insert into small select i, 'v' || (i % 10) from generate_series(1, 1000) i");
    let plan = run(&db, "explain select count(*) from small where v = 'v3'");
    assert!(plan.contains("search index"), "{plan}");
    // about 40 MB of rows: over the default budget (CHRONOS_WORK_MEM / 8 = 32 MB)
    run(&db, "create table big (id int primary key, v text)");
    for c in 0..8 {
        let (a, b) = (c * 5000 + 1, c * 5000 + 5000);
        run(&db, &format!("insert into big select i, repeat(md5(i::text), 32) from generate_series({a}, {b}) i"));
    }
    run(&db, "update big set v = 'changed' where id between 1 and 10");
    let plan = run(&db, "explain select count(*) from big where v = 'changed'");
    assert!(plan.contains("Seq Scan"), "{plan}");
    let out = run(&db, "select count(*) from big where v = 'changed'");
    assert!(out.contains("Int(10)"), "{out}");
}

#[test]
fn statement_timeout_stops_a_runaway_query() {
    let db = Db::new();
    let mut s = sql::Session::new(MAIN);
    let mut q = |text: &str| s.run(&db, text, &[]);
    q("set statement_timeout = '200ms'").unwrap();
    assert!(format!("{:?}", q("show statement_timeout").unwrap()).contains("200ms"));
    let t = std::time::Instant::now();
    let e = q("select count(*) from generate_series(1, 20000) a, generate_series(1, 20000) b").unwrap_err();
    assert_eq!(e.code, "57014", "{e:?}");
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "stopped after {:?}", t.elapsed());
    assert!(format!("{:?}", q("select 1 + 1").unwrap()).contains("Int(2)"), "the session goes on");
    q("reset statement_timeout").unwrap();
    assert!(format!("{:?}", q("show statement_timeout").unwrap()).contains("Text(\"0\")"));
    q("set statement_timeout to 5000").unwrap();
    assert!(format!("{:?}", q("show statement_timeout").unwrap()).contains("5s"));
    assert!(q("set statement_timeout = 'soon'").is_err());
}

/// Two subqueries reading the outer query's aggregate (`sum(g.a)`, lifted), each in a function body planned for
/// each call, with the same outer and lifted values: each gets its own answer. Their answers were cached by the
/// plan's address, which the second call's plan could reuse, and then got the first one's rows (TODOS: "A lifted
/// subquery's answers are cached by its plan's address").
#[test]
fn lifted_subqueries_in_function_bodies_keep_their_own_answers() {
    let db = Db::new();
    for q in [
        "create table g (a int primary key)",
        "insert into g values (1), (2)",
        "create function three() returns bigint language sql as \
         $$ select count(*) from g having exists (select 1 where sum(g.a) = 3) $$",
        "create function four() returns bigint language sql as \
         $$ select count(*) from g having exists (select 1 where sum(g.a) = 4) $$",
        "create function three_pl() returns bigint language plpgsql as \
         $$ begin return (select count(*) from g having exists (select 1 where sum(g.a) = 3)); end $$",
        "create function four_pl() returns bigint language plpgsql as \
         $$ begin return (select count(*) from g having exists (select 1 where sum(g.a) = 4)); end $$",
    ] {
        sql::run(&db, MAIN, q, &[]).unwrap_or_else(|e| panic!("{q}: {e}"));
    }
    let rows = |q: &str| -> Vec<String> {
        let out = sql::run(&db, MAIN, q, &[]).unwrap_or_else(|e| panic!("{q}: {e}")).pop().unwrap();
        out.rows.iter().map(|r| r.iter().map(|v| v.to_text()).collect::<Vec<_>>().join("|")).collect()
    };
    let want = vec!["2|".to_string(); 4]; // (a null is empty text)
    for q in [
        "select three(), four() from generate_series(1, 4)",
        "select three_pl(), four_pl() from generate_series(1, 4)",
    ] {
        assert_eq!(rows(q), want, "{q}");
    }
    assert_eq!(rows("select four(), three() from generate_series(1, 2)"), vec!["|2".to_string(); 2]);
    // two lifted aggregates with equal values but other types (Codex on #462)
    sql::run(&db, MAIN, "create table gt (a int, b bigint)", &[]).unwrap();
    sql::run(&db, MAIN, "insert into gt values (1, 1)", &[]).unwrap();
    assert_eq!(
        rows("select (select pg_typeof(min(gt.a))::text), (select pg_typeof(min(gt.b))::text) from gt"),
        vec!["integer|bigint".to_string()]
    );
    // equal values that look different: each group's own (Devin on #462)
    sql::run(&db, MAIN, "create table gi (k int, i interval, n numeric)", &[]).unwrap();
    sql::run(&db, MAIN, "insert into gi values (1, '1 month', 1.0), (2, '30 days', 1.00)", &[]).unwrap();
    assert_eq!(
        rows("select k, (select max(gi.i)::text), (select max(gi.n)::text) from gi group by k order by k"),
        vec!["1|1 mon|1.0".to_string(), "2|30 days|1.00".to_string()]
    );
    // the outer columns' types too (Codex on #462): one smallint, one integer, same rows
    sql::run(&db, MAIN, "create table ga (a int, s smallint)", &[]).unwrap();
    sql::run(&db, MAIN, "create table gb (a int, s int)", &[]).unwrap();
    sql::run(&db, MAIN, "insert into ga values (1, 7)", &[]).unwrap();
    sql::run(&db, MAIN, "insert into gb values (1, 7)", &[]).unwrap();
    assert_eq!(
        rows("select (select pg_typeof(g.s)::text where sum(g.a) = 1) from ga g group by g.s \
              union all select (select pg_typeof(g.s)::text where sum(g.a) = 1) from gb g group by g.s"),
        vec!["smallint".to_string(), "integer".to_string()]
    );
}

/// A STRICT SQL function in a CHECK or a generated column, its parameter a domain: a NULL argument is made the
/// domain's before the STRICT test, so the domain's NOT NULL and CHECK still apply, as Postgres coerces it at the call
/// (TODOS: "A STRICT SQL function in a CHECK or generated column with a domain parameter"). It skipped them.
#[test]
fn a_strict_functions_domain_parameter_is_checked_inline() {
    let db = Db::new();
    for q in [
        "create domain nn as int not null",
        "create domain pos as int check (value > 0)",
        "create function ok_nn(nn) returns boolean language sql strict as $$ select $1 > -1000 $$",
        "create function twice(pos) returns int language sql strict as $$ select $1 * 2 $$",
        "create table c (id int primary key, x int check (ok_nn(x)))",
        "create table gc (id int primary key, x int, y int generated always as (twice(x)) stored)",
    ] {
        sql::run(&db, MAIN, q, &[]).unwrap_or_else(|e| panic!("{q}: {e}"));
    }
    let fails = |q: &str| sql::run(&db, MAIN, q, &[]).map(|_| ()).expect_err(q);
    let e = fails("insert into c values (1, null)");
    assert_eq!((e.code, e.message.as_str()), ("23502", "domain nn does not allow null values"));
    let e = fails("insert into gc values (1, -1)");
    assert_eq!(e.code, "23514", "{e}");
    sql::run(&db, MAIN, "insert into c values (2, 5)", &[]).unwrap();
    sql::run(&db, MAIN, "insert into gc values (2, 3), (3, null)", &[]).unwrap();
    let out = sql::run(&db, MAIN, "select id, y from gc order by id", &[]).unwrap().pop().unwrap();
    let got: Vec<String> = out.rows.iter().map(|r| r.iter().map(|v| v.to_text()).collect::<Vec<_>>().join("|")).collect();
    assert_eq!(got, ["2|6", "3|"]);
}
