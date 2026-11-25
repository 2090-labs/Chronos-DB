//! A UNIQUE constraint or an index a DO block or function makes is there for what the body does next, as in Postgres:
//! GoTrue's migration adds `refresh_tokens_token_unique` and then a foreign key to it in one DO block.

use chronos::sql::{Session, Value};
use chronos::{Db, MAIN};

fn run(db: &Db, s: &mut Session, q: &str) -> Vec<Vec<String>> {
    let out = s.run(db, q, &[]).unwrap_or_else(|e| panic!("{q}: {} {}", e.code, e.message));
    out.last().unwrap().rows.iter().map(|r| r.iter().map(Value::to_text).collect()).collect()
}

#[test]
fn a_foreign_key_to_a_unique_made_just_before() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(
        &db,
        &mut s,
        "CREATE TABLE rt (id int4 PRIMARY KEY, token varchar(255), parent varchar(255));
         INSERT INTO rt VALUES (1, 'a', NULL);
         DO $$
         BEGIN
           IF NOT EXISTS (SELECT * FROM information_schema.constraint_column_usage
                          WHERE table_name = 'rt' AND constraint_name = 'rt_token_unique') THEN
             ALTER TABLE rt ADD CONSTRAINT rt_token_unique UNIQUE (token);
           END IF;
           ALTER TABLE rt ADD CONSTRAINT rt_parent_fkey FOREIGN KEY (parent) REFERENCES rt (token);
           CREATE UNIQUE INDEX rt_id_token ON rt (id, token);
           RAISE NOTICE '%', (SELECT count(*) FROM pg_class WHERE relname = 'rt_id_token');
         END $$",
    );
    assert_eq!(
        run(&db, &mut s, "SELECT conname FROM pg_constraint WHERE conrelid = 'rt'::regclass ORDER BY 1"),
        [["rt_parent_fkey"], ["rt_pkey"], ["rt_token_unique"]]
    );
    assert_eq!(s.run(&db, "INSERT INTO rt VALUES (2, 'b', 'zz')", &[]).unwrap_err().code, "23503");
    assert_eq!(s.run(&db, "INSERT INTO rt VALUES (2, 'a', NULL)", &[]).unwrap_err().code, "23505");
    run(&db, &mut s, "INSERT INTO rt VALUES (2, 'b', 'a')");
}

#[test]
fn rows_the_body_wrote_still_count() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(&db, &mut s, "CREATE TABLE t (a int4)");
    // the body's own rows are in the build: a duplicate among them refuses the constraint
    let e = s
        .run(&db, "DO $$ BEGIN INSERT INTO t VALUES (1), (1); ALTER TABLE t ADD CONSTRAINT t_a UNIQUE (a); END $$", &[])
        .unwrap_err();
    assert_eq!(e.code, "23505");
    run(&db, &mut s, "DO $$ BEGIN INSERT INTO t VALUES (1), (2); ALTER TABLE t ADD CONSTRAINT t_a UNIQUE (a); END $$");
    assert_eq!(s.run(&db, "INSERT INTO t VALUES (2)", &[]).unwrap_err().code, "23505");
}

#[test]
fn a_partitioned_tables_index_counts_its_partitions_rows() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(
        &db,
        &mut s,
        "CREATE TABLE p (a int4) PARTITION BY RANGE (a); CREATE TABLE p1 PARTITION OF p FOR VALUES FROM (0) TO (10)",
    );
    let e = s
        .run(&db, "DO $$ BEGIN INSERT INTO p VALUES (1), (1); CREATE UNIQUE INDEX p_a ON p (a); END $$", &[])
        .unwrap_err();
    assert_eq!(e.code, "23505");
    run(&db, &mut s, "DO $$ BEGIN INSERT INTO p VALUES (2); CREATE UNIQUE INDEX p_a ON p (a); END $$");
    assert_eq!(s.run(&db, "INSERT INTO p VALUES (2)", &[]).unwrap_err().code, "23505");
}

#[test]
fn a_partitions_unique_after_an_index_on_its_parent() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(
        &db,
        &mut s,
        "CREATE TABLE p (id int4, k int4) PARTITION BY RANGE (id);
         CREATE TABLE p1 PARTITION OF p FOR VALUES FROM (0) TO (100);
         DO $$
         BEGIN
           CREATE INDEX p_k ON p (k);
           ALTER TABLE p1 ADD CONSTRAINT p1_k_key UNIQUE (k);
         END $$",
    );
    // both are there after the block: the partition's index and its UNIQUE
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_indexes WHERE tablename = 'p1'"), [["2"]]);
    run(&db, &mut s, "INSERT INTO p VALUES (1, 7)");
    assert_eq!(s.run(&db, "INSERT INTO p VALUES (2, 7)", &[]).unwrap_err().code, "23505");
}

#[test]
fn an_index_name_pending_on_another_table_is_taken() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(&db, &mut s, "CREATE TABLE a (x int4); CREATE TABLE b (x int4)");
    let e = s
        .run(
            &db,
            "DO $$ BEGIN INSERT INTO a VALUES (1); CREATE INDEX shared_idx ON a (x); CREATE INDEX shared_idx ON b (x); END $$",
            &[],
        )
        .unwrap_err();
    assert_eq!(e.code, "42P07", "{}", e.message);
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_indexes WHERE indexname = 'shared_idx'"), [["0"]]);
}

#[test]
fn a_block_rolled_back_takes_its_unique_with_it() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(&db, &mut s, "CREATE TABLE a (id int4 PRIMARY KEY, n int4); INSERT INTO a VALUES (1, 1)");
    // an EXCEPTION handler's block: what it built is undone, what the body did before stays
    run(
        &db,
        &mut s,
        "DO $$ BEGIN
           CREATE INDEX a_id_n ON a (id, n);
           BEGIN
             ALTER TABLE a ADD CONSTRAINT a_n_uq UNIQUE (n);
             RAISE EXCEPTION 'no';
           EXCEPTION WHEN OTHERS THEN NULL;
           END;
         END $$",
    );
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_constraint WHERE conname = 'a_n_uq'"), [["0"]]);
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_indexes WHERE indexname = 'a_id_n'"), [["1"]]);
    run(&db, &mut s, "INSERT INTO a VALUES (2, 1)");
    // a statement that fails: none of it
    assert!(s.run(&db, "DO $$ BEGIN CREATE UNIQUE INDEX a_n2 ON a (id); RAISE EXCEPTION 'no'; END $$", &[]).is_err());
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_indexes WHERE indexname = 'a_n2'"), [["0"]]);
}

/// The rest of a body's DDL goes with its block too (TODOS, engine #283): tables, columns, constraints, functions,
/// a failing DO block's and an EXCEPTION handler's alike, as Postgres undoes them with the block's rows.
#[test]
fn a_bodys_ddl_is_undone_with_its_block() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(&db, &mut s, "CREATE TABLE b (id int4 PRIMARY KEY, n int4); INSERT INTO b VALUES (1, 1)");
    // a DO block that fails: nothing it made stays
    let e = s
        .run(
            &db,
            "DO $$ BEGIN
               CREATE TABLE gone (x int);
               INSERT INTO gone VALUES (1);
               ALTER TABLE b ADD COLUMN extra text;
               ALTER TABLE b ADD CONSTRAINT b_n_pos CHECK (n > 0);
               CREATE FUNCTION gone_fn() RETURNS int LANGUAGE sql AS 'SELECT 1';
               RAISE EXCEPTION 'no';
             END $$",
            &[],
        )
        .unwrap_err();
    assert_eq!(e.message, "no");
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_class WHERE relname = 'gone'"), [["0"]]);
    assert_eq!(
        run(&db, &mut s, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'b' AND column_name = 'extra'"),
        [["0"]]
    );
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_constraint WHERE conname = 'b_n_pos'"), [["0"]]);
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_proc WHERE proname = 'gone_fn'"), [["0"]]);
    run(&db, &mut s, "CREATE TABLE gone (y text)"); // (the name is free again)
    run(&db, &mut s, "INSERT INTO b VALUES (2, -1)"); // (no CHECK)
    // an EXCEPTION handler's block: its DDL is undone, the body's before it stays
    run(
        &db,
        &mut s,
        "DO $$ BEGIN
           CREATE TABLE kept (x int);
           BEGIN
             CREATE TABLE dropped (x int);
             ALTER TABLE kept ADD COLUMN y int;
             DROP TABLE gone;
             RAISE EXCEPTION 'no';
           EXCEPTION WHEN OTHERS THEN NULL;
           END;
           INSERT INTO kept VALUES (1);
         END $$",
    );
    assert_eq!(run(&db, &mut s, "SELECT * FROM kept"), [["1"]]);
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_class WHERE relname = 'dropped'"), [["0"]]);
    assert_eq!(
        run(&db, &mut s, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'kept' AND column_name = 'y'"),
        [["0"]]
    );
    run(&db, &mut s, "INSERT INTO gone VALUES ('still here')");
    assert_eq!(run(&db, &mut s, "SELECT * FROM gone"), [["still here"]]);
    // a function's DDL, from a statement that fails after it
    run(
        &db,
        &mut s,
        "CREATE FUNCTION mk() RETURNS int LANGUAGE plpgsql AS $$ BEGIN CREATE TABLE made (x int); RETURN 1; END $$",
    );
    assert!(s.run(&db, "SELECT mk() / 0", &[]).is_err());
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_class WHERE relname = 'made'"), [["0"]]);
    // and one that succeeds keeps it
    run(&db, &mut s, "SELECT mk()");
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM pg_class WHERE relname = 'made'"), [["1"]]);
    // a serial table dropped in a block rolled back keeps its counter (Devin on #464)
    run(&db, &mut s, "CREATE TABLE ser (id serial PRIMARY KEY, v text); INSERT INTO ser (v) VALUES ('a'), ('b')");
    run(
        &db,
        &mut s,
        "DO $$ BEGIN
           BEGIN DROP TABLE ser; RAISE EXCEPTION 'no'; EXCEPTION WHEN OTHERS THEN NULL; END;
         END $$",
    );
    run(&db, &mut s, "INSERT INTO ser (v) VALUES ('c')");
    assert_eq!(run(&db, &mut s, "SELECT id FROM ser WHERE v = 'c'"), [["3"]]);
    // dropped and made anew in one block: the new table counts from 1, and goes on from what it took (Codex on #464)
    run(
        &db,
        &mut s,
        "DO $$ BEGIN DROP TABLE ser; CREATE TABLE ser (id serial PRIMARY KEY, v text); INSERT INTO ser (v) VALUES ('x'); END $$",
    );
    run(&db, &mut s, "INSERT INTO ser (v) VALUES ('y')");
    assert_eq!(run(&db, &mut s, "SELECT id, v FROM ser ORDER BY id"), [["1", "x"], ["2", "y"]]);
    // dropped, made anew and used in a block rolled back: the table and its counter as they were (Codex on #464)
    run(
        &db,
        &mut s,
        "DO $$ BEGIN
           BEGIN
             DROP TABLE ser; CREATE TABLE ser (id serial PRIMARY KEY, v text); INSERT INTO ser (v) VALUES ('z');
             RAISE EXCEPTION 'no';
           EXCEPTION WHEN OTHERS THEN NULL;
           END;
         END $$",
    );
    run(&db, &mut s, "INSERT INTO ser (v) VALUES ('w')");
    // (the values the undone insert took are skipped, as in Postgres: the next is past them, no row's again)
    assert_eq!(run(&db, &mut s, "SELECT id, v FROM ser WHERE v <> 'w' ORDER BY id"), [["1", "x"], ["2", "y"]]);
    assert_eq!(run(&db, &mut s, "SELECT count(*) FROM ser WHERE v = 'w' AND id > 2"), [["1"]]);
    // a function replaced in a block rolled back is the old one again for the rest of the body (Codex on #464)
    run(&db, &mut s, "CREATE FUNCTION one() RETURNS int LANGUAGE sql AS 'SELECT 1'");
    run(
        &db,
        &mut s,
        "DO $$ DECLARE x int; BEGIN
           BEGIN
             CREATE OR REPLACE FUNCTION one() RETURNS int LANGUAGE sql AS 'SELECT 2';
             x := one();
             RAISE EXCEPTION 'no';
           EXCEPTION WHEN OTHERS THEN NULL;
           END;
           x := one();
           RAISE NOTICE 'one: %', x;
           IF x <> 1 THEN RAISE EXCEPTION 'one() is %', x; END IF;
         END $$",
    );
    assert_eq!(run(&db, &mut s, "SELECT one()"), [["1"]]);
}

#[test]
fn a_blocks_rows_go_back_whatever_its_worlds_budget() {
    // (putting an undone block's writes back is charged to no budget: the DROP used it up, and the table must come
    // back all the same, Codex on #464)
    let db = Db::new();
    let mut s = Session::new(MAIN);
    run(&db, &mut s, "CREATE TABLE two (id int PRIMARY KEY); INSERT INTO two VALUES (1), (2)");
    run(&db, &mut s, "CREATE WORLD bw WITH (budget_rows = 4)");
    let mut w = Session::new("bw");
    run(
        &db,
        &mut w,
        "DO $$ BEGIN
           BEGIN DROP TABLE two; RAISE EXCEPTION 'no'; EXCEPTION WHEN OTHERS THEN NULL; END;
         END $$",
    );
    assert_eq!(run(&db, &mut w, "SELECT id FROM two ORDER BY id"), [["1"], ["2"]]);
}
