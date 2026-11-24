//! A merge policy's reviewer: an agent (a reviewer model's) that decides held merges before a person. It reads the
//! held world as an agent does (masked columns masked), and approves (the merge goes through, at the version that was
//! held), rejects, or passes it to a person, with a reason kept in SHOW REVIEWS and the audit. Issue #46.

use chronos::sql::{Session, run};
use chronos::{AgentSpec, Can, Db, MAIN};

fn agent(db: &Db, name: &str, can: &[Can], policy: Option<&str>) -> Session {
    let spec = AgentSpec { can: Some(can.to_vec()), policy: policy.map(String::from), ..Default::default() };
    db.create_agent(name, &spec).unwrap();
    let mut s = Session::new(MAIN);
    s.agent = Some(name.into());
    s
}

fn reviews(db: &Db) -> Vec<(String, String)> {
    let out = run(db, MAIN, "show reviews", &[]).unwrap().pop().unwrap();
    let cols = out.columns.as_ref().unwrap();
    let at = |n: &str| cols.iter().position(|(c, _)| c == n).unwrap();
    out.rows.iter().map(|r| (r[at("world")].to_text(), r[at("reviewer")].to_text())).collect()
}

#[test]
fn a_reviewer_decides_before_a_person() {
    let dir = std::env::temp_dir().join(format!("chronos-policy-reviewer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Db::open(&dir).unwrap();
    run(
        &db,
        MAIN,
        "create table leads (id int primary key, email text, status text);
         insert into leads select g, 'p' || g || '@x.com', 'open' from generate_series(1, 5) g;
         alter table leads alter column email set masked;
         create merge policy careful with (max_deletes = 0, reviewer = 'critic', person_only = 'none')",
        &[],
    )
    .unwrap();
    let writer = [Can::Read, Can::Fork, Can::WriteOwn, Can::WriteMain, Can::MergeOwn];
    let mut bot = agent(&db, "bot", &writer, Some("careful"));
    let mut critic = agent(&db, "critic", &[Can::Read], None);
    let mut other = agent(&db, "other", &[Can::Read], None);
    // (person_only = 'none': the reviewer may approve deletes too, which by default only a person may)
    let shown = run(&db, MAIN, "show merge policies", &[]).unwrap().pop().unwrap();
    assert!(shown.rows[0].iter().any(|v| v.to_text() == "critic"));

    // three held merges
    for (w, sql) in [
        ("a", "delete from leads where id = 1"),
        ("b", "delete from leads where id = 2"),
        ("c", "delete from leads where id = 3"),
    ] {
        bot.run(&db, &format!("create world {w}; use world {w}; {sql}; use world main"), &[]).unwrap();
        assert!(bot.run(&db, &format!("merge world {w}"), &[]).is_err(), "held");
    }
    assert!(reviews(&db).iter().all(|(_, r)| r == "waiting for critic"), "{:?}", reviews(&db));

    // the reviewer reads what an agent may: the diff, masked
    let diff = critic.run(&db, "diff world a", &[]).unwrap().pop().unwrap();
    assert!(!format!("{:?}", diff.rows).contains("p1@x.com"), "masked: {:?}", diff.rows);

    // only it (or an admin, or a person) decides
    let e = other.run(&db, "review world a approve reason 'looks fine'", &[]).unwrap_err();
    assert_eq!(e.code, "42501", "{}", e.message);
    assert!(e.message.contains("isn't merge policy careful's reviewer"), "{}", e.message);

    // approve: merged, as held
    let out = critic.run(&db, "review world a approve reason 'a duplicate of lead 4'", &[]).unwrap();
    assert_eq!(out[0].tag, "REVIEW APPROVE 1");
    assert!(db.world("a").is_err(), "merged");
    assert_eq!(run(&db, MAIN, "select count(*) from leads where id = 1", &[]).unwrap()[0].rows[0][0].to_text(), "0");
    // reject and pass: not merged, the reason shown for a person
    critic.run(&db, "review world b reject reason 'lead 2 has an open deal'", &[]).unwrap();
    critic.run(&db, "review world c pass reason 'not sure: check the deal'", &[]).unwrap();
    let r = reviews(&db);
    assert!(r.contains(&("b".into(), "rejected by critic: lead 2 has an open deal".into())), "{r:?}");
    assert!(r.contains(&("c".into(), "passed to a person by critic: not sure: check the deal".into())), "{r:?}");
    // a person still decides what's left
    run(&db, MAIN, "merge world c", &[]).unwrap();

    // a world written to since it was held isn't approved: what's merged must be what was reviewed
    bot.run(&db, "use world b; update leads set status = 'x' where id = 5; use world main", &[]).unwrap();
    let e = critic.run(&db, "review world b approve", &[]).unwrap_err();
    assert_eq!(e.code, "40001", "{}", e.message);
    // nor is a world that isn't held
    run(&db, MAIN, "create world free", &[]).unwrap();
    assert_eq!(critic.run(&db, "review world free approve", &[]).unwrap_err().code, "55000");

    // the audit says who decided what, and why
    let audit = run(&db, MAIN, "show audit for agent critic", &[]).unwrap().pop().unwrap();
    let actions: Vec<String> = audit.rows.iter().map(|r| r[3].to_text()).collect();
    assert!(actions.contains(&"review approved: a duplicate of lead 4".to_string()), "{actions:?}");
    assert!(actions.contains(&"review rejected: lead 2 has an open deal".to_string()), "{actions:?}");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// No agent decides the review of its own world, whatever it may otherwise decide: not the admin agent whose merge
/// was held, nor the policy's reviewer when the held world is its own. Another reviewer, another admin agent or a
/// person does. As for proposals ("can't decide its own proposal"). Engine #404.
#[test]
fn an_agent_does_not_decide_its_own_worlds_review() {
    let dir = std::env::temp_dir().join(format!("chronos-policy-reviewer-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Db::open(&dir).unwrap();
    run(
        &db,
        MAIN,
        "create table leads (id int primary key, status text);
         insert into leads select g, 'open' from generate_series(1, 9) g;
         create merge policy careful with (max_deletes = 0, reviewer = 'critic', person_only = 'none')",
        &[],
    )
    .unwrap();
    let writer = [Can::Read, Can::Fork, Can::WriteOwn, Can::WriteMain, Can::MergeOwn];
    let admin = [Can::Read, Can::Fork, Can::WriteOwn, Can::WriteMain, Can::MergeOwn, Can::Admin];
    let mut boss = agent(&db, "boss", &admin, Some("careful"));
    let mut critic = agent(&db, "critic", &writer, Some("careful"));
    let mut chief = agent(&db, "chief", &admin, None);
    let hold = |s: &mut Session, w: &str, id: i32| {
        s.run(&db, &format!("create world {w}; use world {w}; delete from leads where id = {id}; use world main"), &[])
            .unwrap();
        let e = s.run(&db, &format!("merge world {w}"), &[]).unwrap_err();
        assert!(reviews(&db).iter().any(|(x, _)| x == w), "{w} held: {}", e.message);
    };
    let own = |e: chronos::sql::SqlError| {
        assert_eq!(e.code, "42501", "{}", e.message);
        assert!(e.message.contains("can't decide the review of its own world"), "{}", e.message);
    };

    // an admin agent may decide any review but its own world's: approve, reject or pass
    hold(&mut boss, "mine", 1);
    for verdict in ["approve", "reject", "pass"] {
        own(boss.run(&db, &format!("review world mine {verdict} reason 'fine by me'"), &[]).unwrap_err());
    }
    // the policy's reviewer doesn't decide its own world either
    hold(&mut critic, "critics", 2);
    own(critic.run(&db, "review world critics approve", &[]).unwrap_err());

    // another admin agent, the reviewer (on a world not its own) and a person still do
    assert_eq!(chief.run(&db, "review world critics approve reason 'checked'", &[]).unwrap()[0].tag, "REVIEW APPROVE 1");
    assert_eq!(critic.run(&db, "review world mine reject reason 'lead 1 has a deal'", &[]).unwrap()[0].tag, "REVIEW REJECT");
    // nor the world's owner when another agent's merge of it was held
    let mut mover = agent(&db, "mover", &[Can::Read, Can::WriteMain, Can::Merge], Some("careful"));
    boss.run(&db, "create world owned; use world owned; delete from leads where id = 4; use world main", &[]).unwrap();
    assert!(mover.run(&db, "merge world owned", &[]).is_err(), "held");
    own(boss.run(&db, "review world owned approve reason 'mine anyway'", &[]).unwrap_err());
    hold(&mut boss, "again", 3);
    run(&db, MAIN, "review world again approve reason 'a person checked'", &[]).unwrap();
    assert_eq!(run(&db, MAIN, "select count(*) from leads", &[]).unwrap()[0].rows[0][0].to_text(), "7");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
