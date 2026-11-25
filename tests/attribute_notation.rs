//! Attribute notation and functional notation, as Postgres reads them (ParseFuncOrColumn): `t.f` is `f(t)` where t
//! has no column f but a function f takes its row, `name(x)` is `x.name` where no function name takes x, and a
//! set-returning function nested in another's arguments in a select list runs first, a level each (misc.sql).

use chronos::sql::{Session, Value};
use chronos::{Db, MAIN};

fn run(db: &Db, s: &mut Session, q: &str) -> Vec<Vec<String>> {
    let out = s.run(db, q, &[]).unwrap_or_else(|e| panic!("{q}: {} {}", e.code, e.message));
    out.last().unwrap().rows.iter().map(|r| r.iter().map(Value::to_text).collect()).collect()
}

fn setup(db: &Db, s: &mut Session) {
    run(
        db,
        s,
        "CREATE TABLE person (name text, age int4);
         INSERT INTO person VALUES ('mike', 40), ('joe', 20);
         CREATE TABLE emp (salary int4) INHERITS (person);
         INSERT INTO emp VALUES ('sally', 34, 1000);
         CREATE TABLE hobbies_r (name text, person text);
         INSERT INTO hobbies_r VALUES ('posthacking', 'mike'), ('basketball', 'joe'), ('basketball', 'sally'),
             ('skywalking', NULL);
         CREATE TABLE equipment_r (name text, hobby text);
         INSERT INTO equipment_r VALUES ('advil', 'posthacking'), ('peet''s coffee', 'posthacking'),
             ('hightops', 'basketball'), ('guts', 'skywalking');
         CREATE FUNCTION hobbies(person) RETURNS setof hobbies_r
             AS 'select * from hobbies_r where person = $1.name' LANGUAGE SQL;
         CREATE FUNCTION hobby_construct(text, text) RETURNS hobbies_r AS 'select $1 as name, $2 as hobby' LANGUAGE SQL;
         CREATE FUNCTION equipment(hobbies_r) RETURNS setof equipment_r
             AS 'select * from equipment_r where hobby = $1.name' LANGUAGE SQL;",
    );
}

#[test]
fn attribute_notation_calls_the_function() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    setup(&db, &mut s);
    assert_eq!(
        run(&db, &mut s, "SELECT p.name, name(p.hobbies) FROM ONLY person p"),
        [["mike", "posthacking"], ["joe", "basketball"]]
    );
    // an inherited table's rows too (person*, and person, which reads its children)
    assert_eq!(run(&db, &mut s, "SELECT p.name, name(p.hobbies) FROM person* p ORDER BY 1").len(), 3);
    assert_eq!(run(&db, &mut s, "SELECT p.name, name(p.hobbies) FROM person p ORDER BY 1").len(), 3);
    // `(t.f).g`, with no call written
    assert_eq!(
        run(&db, &mut s, "SELECT (hobbies_r.equipment).name FROM hobbies_r WHERE hobbies_r.name = 'basketball'"),
        [["hightops"], ["hightops"]]
    );
    // a column of the name is always the column
    assert_eq!(run(&db, &mut s, "SELECT h.name FROM hobbies_r h WHERE h.person = 'mike'"), [["posthacking"]]);
}

#[test]
fn nested_set_returning_calls_run_a_level_each() {
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    setup(&db, &mut s);
    assert_eq!(
        run(&db, &mut s, "SELECT p.name, name(p.hobbies), name(equipment(p.hobbies)) FROM ONLY person p"),
        [["mike", "posthacking", "advil"], ["mike", "posthacking", "peet's coffee"], ["joe", "basketball", "hightops"]]
    );
    assert_eq!(
        run(&db, &mut s, "SELECT (p.hobbies).equipment.name, p.name FROM ONLY person p WHERE p.name = 'joe'"),
        [["hightops", "joe"]]
    );
    assert_eq!(run(&db, &mut s, "SELECT name(equipment(hobby_construct(text 'skywalking', text 'mer')))"), [["guts"]]);
    assert_eq!(run(&db, &mut s, "SELECT name(equipment(ROW('skywalking', 'mer')))"), [["guts"]]);
    // (a schema-qualified call is a call: `missing.name(x)` is never `x.name`, misc.sql)
    match s.run(&db, "SELECT missing.name(equipment(ROW('skywalking', 'mer')))", &[]) {
        Ok(_) => panic!("a schema-qualified call read as a field"),
        Err(e) => assert!(e.message.contains("missing"), "{} {}", e.code, e.message),
    }
    // nor one by a schema its stored name drops (public, pg_temp, pg_catalog: Devin on #469)
    for q in ["public", "pg_temp", "pg_catalog"] {
        match s.run(&db, &format!("SELECT {q}.name(equipment(ROW('skywalking', 'mer')))"), &[]) {
            Ok(r) => panic!("{q}.name(x) read as a field: {r:?}"),
            Err(e) => assert!(e.message.contains("name"), "{q}: {} {}", e.code, e.message),
        }
    }
    // (but `f(x)` and `public.f(x)` are the same call: a GROUP BY key matches either, Codex on #469)
    run(&db, &mut s, "create table gq (x int)");
    run(&db, &mut s, "insert into gq values (1), (1), (2)");
    run(&db, &mut s, "create function gqf(int) returns int language plpgsql as $$ begin return $1 * 10; end $$");
    assert_eq!(run(&db, &mut s, "select gqf(x) from gq group by public.gqf(x) order by 1"), [["10"], ["20"]]);
    // `*` is the FROM items' columns, not the call's own source's
    assert_eq!(
        run(&db, &mut s, "SELECT *, name(equipment(h.*)) FROM hobbies_r h WHERE h.name = 'skywalking'"),
        [["skywalking", "", "guts"]]
    );

    // a column alias names the column: `p.f` is it, not f(p)
    run(
        &db,
        &mut s,
        "CREATE TABLE t (x int4); INSERT INTO t VALUES (7); CREATE FUNCTION f(int4) RETURNS int4 AS 'select $1' LANGUAGE SQL",
    );
    assert_eq!(run(&db, &mut s, "SELECT p.f, f(1) FROM t AS p(f)"), [["7", "1"]]);
    // and an alias of the user's, whatever it starts with, keeps its columns in `*`
    assert_eq!(run(&db, &mut s, "SELECT * FROM t AS \"?usrf_data\""), [["7"]]);

    // the call is given the row, though the row has a column of its own name
    run(
        &db,
        &mut s,
        "CREATE TABLE r (r int4); INSERT INTO r VALUES (7);
         CREATE FUNCTION twice(r) RETURNS int4 AS 'select $1.r * 2' LANGUAGE SQL",
    );
    assert_eq!(run(&db, &mut s, "SELECT r.twice, twice(r.*) FROM r"), [["14", "14"]]);

    // `(g(x)).a` of a function with overloads whose rows differ is not made a(g(x)) (reading the field of an
    // overloaded call's row is a gap of its own: it may fail, but it never calls a)
    run(
        &db,
        &mut s,
        "CREATE TABLE ra (a int4); CREATE TABLE rb (b int4);
         CREATE FUNCTION g(int4) RETURNS ra AS 'select $1' LANGUAGE SQL;
         CREATE FUNCTION g(text) RETURNS rb AS 'select 5' LANGUAGE SQL;
         CREATE FUNCTION a(rb) RETURNS int4 AS 'select 99' LANGUAGE SQL",
    );
    let out = s.run(&db, "SELECT (g(1)).a", &[]);
    assert!(out.map_or(true, |o| o.last().unwrap().rows[0][0].to_text() != "99"));
}

#[test]
fn attribute_notation_finds_its_function_along_the_search_path() {
    // (`p.f` is `f(p)`, the function `f(p.*)` written out calls: the search_path's first, not public's)
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    run(
        &db,
        &mut s,
        "CREATE TABLE pt (a int4);
         INSERT INTO pt VALUES (1);
         CREATE SCHEMA s;
         CREATE FUNCTION s.f(pt) RETURNS text AS $$ SELECT 's' $$ LANGUAGE SQL;
         CREATE FUNCTION public.f(pt) RETURNS text AS $$ SELECT 'public' $$ LANGUAGE SQL;
         SET search_path = s, public",
    );
    assert_eq!(run(&db, &mut s, "SELECT f(p.*) FROM pt p"), [["s"]]);
    // (with a call beside it: a statement of `p.f` alone isn't read for calls yet, see TODOS)
    assert_eq!(run(&db, &mut s, "SELECT p.f, f(p.*) FROM pt p"), [["s", "s"]]);
    run(&db, &mut s, "SET search_path = public, s");
    assert_eq!(run(&db, &mut s, "SELECT p.f, f(p.*) FROM pt p"), [["public", "public"]]);
}

#[test]
fn attribute_notation_leaves_out_functions_the_search_path_hides() {
    // (a function later on the search_path with the same argument types is hidden, as the call's resolver hides it:
    // it neither makes the chain ambiguous nor stops a set-returning call's rows, Devin on #470)
    let (db, mut s) = (Db::new(), Session::new(MAIN));
    setup(&db, &mut s);
    run(
        &db,
        &mut s,
        "CREATE SCHEMA s;
         CREATE FUNCTION s.hobbies(person) RETURNS setof hobbies_r AS 'select * from hobbies_r' LANGUAGE SQL;
         SET search_path = public, s",
    );
    assert_eq!(
        run(&db, &mut s, "SELECT (p.hobbies).equipment.name FROM ONLY person p WHERE p.name = 'joe'"),
        [["hightops"]]
    );
    run(
        &db,
        &mut s,
        "DROP FUNCTION s.hobbies(person);
         CREATE FUNCTION s.hobbies(person) RETURNS text AS $$ select 'scalar' $$ LANGUAGE SQL",
    );
    assert_eq!(run(&db, &mut s, "SELECT (p.hobbies).name FROM ONLY person p WHERE p.name = 'joe'"), [["basketball"]]);
    assert_eq!(run(&db, &mut s, "SELECT name(hobbies(p)) FROM ONLY person p WHERE p.name = 'joe'"), [["basketball"]]);
}
