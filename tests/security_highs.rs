//! What a role with little may not do to get more (the 2026-11-10 security review): make or drop a rule on a table
//! that isn't its own, whose actions would run as whoever writes the table next; rotate an agent's token, which
//! hands it the agent; or end up a superuser because its role was dropped while its login was under way.

use chronos::sql::Session;
use chronos::{Db, MAIN};

fn ok(db: &Db, s: &mut Session, sql: &str) {
    s.run(db, sql, &[]).unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// The one value `sql` gives.
fn one(db: &Db, s: &mut Session, sql: &str) -> String {
    let out = s.run(db, sql, &[]).unwrap_or_else(|e| panic!("{sql}: {e}")).pop().unwrap();
    out.rows[0][0].to_text()
}

/// The SQLSTATE `sql` is refused with.
fn refused(db: &Db, s: &mut Session, sql: &str) -> &'static str {
    match s.run(db, sql, &[]) {
        Ok(_) => panic!("{sql}: ran, and should have been refused"),
        Err(e) => e.code,
    }
}

/// A session logged in as role `name`.
fn role(db: &Db, name: &str) -> Session {
    let mut s = Session::new(MAIN);
    s.user = Some(name.into());
    s.log_in(db, name).unwrap();
    s
}

#[test]
fn a_rule_is_its_tables_owners_to_make_drop_and_switch() {
    let (db, mut admin) = (Db::new(), Session::new(MAIN));
    ok(
        &db,
        &mut admin,
        "create role low login;
         create table orders (id int primary key, card text);
         grant select, insert, update, delete on orders to low;
         grant create on schema public to low;
         create rule guard as on delete to orders do instead nothing;
         insert into orders values (1, '4111')",
    );
    let mut low = role(&db, "low");
    // not on a table it only has privileges on: its actions would run as the next role to write the table
    for sql in [
        "create rule z as on insert to orders do also alter role low superuser",
        "create rule z as on update to orders do instead nothing",
        "create or replace rule guard as on delete to orders do also delete from orders",
        "drop rule guard on orders",
        "alter table orders disable rule guard",
        "alter table orders enable rule guard",
    ] {
        assert_eq!(refused(&db, &mut low, sql), "42501", "{sql}");
    }
    // the owner's rule is as it was: it still takes the delete, and no other was added
    ok(&db, &mut admin, "delete from orders");
    assert_eq!(one(&db, &mut admin, "select count(*) from orders"), "1");
    ok(&db, &mut admin, "insert into orders values (2, '4242')");
    assert_eq!(one(&db, &mut admin, "select rolsuper from pg_roles where rolname = 'low'"), "false");
    // on a table of its own, a rule is its to make, switch and drop
    ok(&db, &mut low, "create table mine (a int)");
    ok(&db, &mut low, "create rule quiet as on insert to mine do instead nothing");
    ok(&db, &mut low, "alter table mine disable rule quiet");
    ok(&db, &mut low, "drop rule quiet on mine");
}

#[test]
fn an_agents_token_is_a_superusers_to_rotate() {
    let (db, mut admin) = (Db::new(), Session::new(MAIN));
    ok(&db, &mut admin, "create role low login; create agent bot with (can = 'read')");
    let mut low = role(&db, "low");
    // (the statement answers with the new token: whoever may run it is the agent from then on)
    assert_eq!(refused(&db, &mut low, "alter agent bot rotate token"), "42501");
    assert_eq!(refused(&db, &mut low, "alter agent nobody rotate token"), "42501", "nor told which agents there are");
    ok(&db, &mut admin, "alter agent bot rotate token");
}

#[test]
fn a_role_dropped_or_changed_while_it_logs_in_is_refused() {
    let (db, mut admin) = (Db::new(), Session::new(MAIN));
    ok(&db, &mut admin, "create role r login password 'first-password-123'");
    // what a login as r proves, read before the exchange (see src/pg.rs)
    let proved = Session::role_password(&db, "r").unwrap().unwrap().0.unwrap();
    let login = |db: &Db| {
        let mut s = Session::new(MAIN);
        s.user = Some("r".into());
        s.log_in_role(db, "r", &proved).map(|()| s)
    };

    // still that role with that password: logged in as itself, with its own rights and no more
    let mut s = login(&db).unwrap();
    assert_eq!(one(&db, &mut s, "select current_user"), "r");
    assert_eq!(refused(&db, &mut s, "create role other"), "42501");

    // given another password meanwhile: what it proved is no longer the role's
    ok(&db, &mut admin, "alter role r password 'second-password-456'");
    assert_eq!(login(&db).map(|_| ()).unwrap_err().code, "28P01");

    // dropped meanwhile: r is a name that is no role now, which a plain login makes a superuser
    ok(&db, &mut admin, "drop role r");
    assert_eq!(login(&db).map(|_| ()).unwrap_err().code, "28P01");

    // and a role of that name made again is another role, with another password
    ok(&db, &mut admin, "create role r login password 'first-password-123'");
    assert_eq!(login(&db).map(|_| ()).unwrap_err().code, "28P01", "a new verifier has a new salt");
}

#[test]
fn a_cast_is_its_types_owners_to_make_and_drop() {
    let (db, mut admin) = (Db::new(), Session::new(MAIN));
    ok(
        &db,
        &mut admin,
        "create role low login;
         grant create on schema public to low;
         create type mood as enum ('ok', 'sad');
         create function mood_len(mood) returns int language sql as $$ select 1 $$;
         create cast (mood as int) with function mood_len(mood)",
    );
    let mut low = role(&db, "low");
    // not between built-in types (the bootstrap superuser's), nor on a type another role owns: a cast changes how
    // every role's values convert, and its function runs as whoever converts them
    ok(&db, &mut low, "create function t2i(text) returns int language sql as $$ select 0 $$");
    for sql in [
        "create cast (text as int) with function t2i(text) as implicit",
        "create cast (mood as text) with inout",
        "drop cast (mood as int)",
    ] {
        assert_eq!(refused(&db, &mut low, sql), "42501", "{sql}");
    }
    assert!(
        low.run(&db, "drop cast (mood as int)", &[]).unwrap_err().message.contains("must be owner of type mood or type integer")
    );
    // (a cast that isn't there: IF EXISTS skips, as for an owner)
    ok(&db, &mut low, "drop cast if exists (mood as bigint)");
    // the owner's cast is still there: its owner drops it
    ok(&db, &mut admin, "drop cast (mood as int)");
    // on a type of its own, a cast is its to make and drop
    ok(&db, &mut low, "create type mine as enum ('a')");
    ok(&db, &mut low, "create cast (mine as text) with inout");
    ok(&db, &mut low, "drop cast (mine as text)");
    // its range's multirange is its own too
    ok(&db, &mut low, "create type own_range as range (subtype = int)");
    ok(&db, &mut low, "create cast (own_multirange as text) with inout; drop cast (own_multirange as text)");
    assert_eq!(refused(&db, &mut low, "create cast (int4multirange as text) with inout"), "42501");
    // a relation named as a built-in type gives no claim on it
    ok(&db, &mut low, "create sequence public.integer");
    assert_eq!(refused(&db, &mut low, "create cast (int as text) with inout"), "42501");
    // (a type whose name holds a comma is still one type)
    ok(&db, &mut low, r#"create type "a,b" as enum ('x')"#);
    ok(&db, &mut low, r#"create cast ("a,b" as text) with inout"#);
    ok(&db, &mut low, r#"drop cast ("a,b" as text)"#);
}

#[test]
fn a_databases_own_statements_are_refused_from_inside_a_function_too() {
    let (db, mut admin) = (Db::new(), Session::new(MAIN));
    ok(&db, &mut admin, "create role low login; grant create on schema public to low");
    let mut low = role(&db, "low");
    // (a function or DO block of an ordinary role is no way round the check: not even a row count shows)
    ok(
        &db,
        &mut low,
        "create function n() returns bigint language plpgsql as $$ declare n bigint;
         begin execute 'show audit'; get diagnostics n = row_count; return n; end $$",
    );
    assert_eq!(refused(&db, &mut low, "select n()"), "42501");
    for sql in ["show audit", "show worlds", "show metrics"] {
        assert_eq!(refused(&db, &mut low, &format!("do $$ begin execute '{sql}'; end $$")), "42501", "{sql}");
    }
}
