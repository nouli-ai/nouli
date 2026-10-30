//! Stable, collision-checked names for sessions without a caller-supplied name.
use super::{Session, SessionError};
use crate::{
    identity::SessionName,
    storage::{StorageError, sql_error},
};
use rusqlite::Connection;

const SCIENTISTS: &[&str] = &[
    "bohr", "born", "bose", "boyle", "curie", "crick", "darwin", "dirac", "euler", "faraday",
    "fermi", "gauss", "hahn", "hertz", "hooke", "hopper", "joule", "kepler", "maxwell", "meitner",
    "mendel", "newton", "noether", "ohm", "pascal", "planck", "raman", "tesla", "turing", "volta",
    "wu", "yang",
];

// Original spinner-style verbs and their noun forms. The verb forms were absent
// from the installed Claude Code 2.1.292/311/312/314 binaries on 2026-10-30.
const WORDS: &[(&str, &str)] = &[
    ("adsorbing", "adsorption"),
    ("alloying", "alloy"),
    ("annealing", "anneal"),
    ("cantilevering", "cantilever"),
    ("collimating", "collimation"),
    ("contouring", "contour"),
    ("diffracting", "diffraction"),
    ("equilibrating", "equilibration"),
    ("filleting", "fillet"),
    ("gridding", "grid"),
    ("interlacing", "interlace"),
    ("knurling", "knurl"),
    ("laminating", "lamination"),
    ("lathing", "lathe"),
    ("modulating", "modulation"),
    ("nocking", "nock"),
    ("notching", "notch"),
    ("oscillating", "oscillation"),
    ("overprinting", "overprint"),
    ("plaiting", "plait"),
    ("polarizing", "polarization"),
    ("refracting", "refraction"),
    ("resonating", "resonance"),
    ("riveting", "rivet"),
    ("sintering", "sinter"),
    ("soldering", "solder"),
    ("tessellating", "tessellation"),
    ("triangulating", "triangulation"),
    ("vectorizing", "vectorization"),
    ("vitrifying", "vitrification"),
    ("winnowing", "winnow"),
];
const NUMBERS: usize = 20;
const CAPACITY: usize = SCIENTISTS.len() * NUMBERS * WORDS.len();

fn candidate(index: usize) -> String {
    let scientist = SCIENTISTS[index / (NUMBERS * WORDS.len())];
    let number = index / WORDS.len() % NUMBERS + 1;
    let noun = WORDS[index % WORDS.len()].1;
    format!("{scientist}-{number}-{noun}")
}

fn available(
    start: usize,
    mut occupied: impl FnMut(&str) -> Result<bool, StorageError>,
) -> Result<SessionName, SessionError> {
    for offset in 0..CAPACITY {
        let name = candidate((start + offset) % CAPACITY);
        if !occupied(&name)? {
            return SessionName::try_from(name).map_err(|_| StorageError::Payload.into());
        }
    }
    Err(SessionError::NamesExhausted)
}

/// Called inside the same immediate transaction as the unique-name insert.
pub(super) fn assign(connection: &Connection, session: &mut Session) -> Result<(), SessionError> {
    if session.name.is_some() {
        return Ok(());
    }
    // UUIDv7's tail is random; avoid its shared timestamp prefix.
    let id = session.id.to_string();
    let seed = u64::from_str_radix(&id[id.len() - 12..], 16).map_err(|_| StorageError::Payload)?;
    let start = (seed % CAPACITY as u64) as usize;
    let mut statement = connection
        .prepare("SELECT EXISTS(SELECT 1 FROM sessions WHERE name=?1)")
        .map_err(|error| sql_error("prepare automatic session name", error))?;
    session.name = Some(available(start, |name| {
        statement
            .query_row([name], |row| row.get(0))
            .map_err(|error| sql_error("check automatic session name", error))
    })?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn generated_names_skip_explicitly_reserved_names_and_survive_reopen() {
        use crate::{
            history::ModelSelection,
            identity::{Id, SessionSelector},
            storage::{Ledger, OpenMode},
        };
        let root = std::env::temp_dir().join(format!("nouli-names-{}", Id::generate()));
        let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
        let id = Id::generate();
        let text = id.to_string();
        let start =
            (u64::from_str_radix(&text[text.len() - 12..], 16).unwrap() % CAPACITY as u64) as usize;
        let reserved = SessionName::try_from(candidate(start)).unwrap();
        ledger
            .create_session(Some(reserved.clone()), ModelSelection::default())
            .unwrap();
        let mut session = Session {
            id,
            name: None,
            defaults: ModelSelection::default(),
            frontier: vec![],
            revision: 0,
        };
        ledger
            .transaction::<_, SessionError>("test generated collision", |tx| {
                super::super::insert_session(tx, &mut session, &[])
            })
            .unwrap();
        let name = session.name.as_ref().unwrap();
        assert_ne!(name, &reserved);
        drop(ledger);
        let ledger = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(
            ledger
                .session(&SessionSelector::Name(name.clone()))
                .unwrap(),
            session
        );
        drop(ledger);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_creators_get_distinct_persisted_names() {
        use crate::{
            history::ModelSelection,
            identity::Id,
            storage::{Ledger, OpenMode},
        };
        let root = std::env::temp_dir().join(format!("nouli-name-race-{}", Id::generate()));
        drop(Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let mut ledger = Ledger::open(&root, OpenMode::ReadWriteCreate).unwrap();
                    (0..12)
                        .map(|_| {
                            ledger
                                .create_session(None, ModelSelection::default())
                                .unwrap()
                                .name
                                .unwrap()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let names: HashSet<_> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(names.len(), 48);
        let ledger = Ledger::open(&root, OpenMode::ReadOnlyExisting).unwrap();
        assert_eq!(ledger.list_sessions().unwrap().len(), 48);
        drop(ledger);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_combination_is_unique_lowercase_and_has_a_number_from_one_to_twenty() {
        let mut seen = HashSet::new();
        for index in 0..CAPACITY {
            let name = candidate(index);
            let parts: Vec<_> = name.split('-').collect();
            assert_eq!(parts.len(), 3);
            assert!(SCIENTISTS.contains(&parts[0]));
            assert!((1..=20).contains(&parts[1].parse::<u8>().unwrap()));
            assert!(WORDS.iter().any(|(_, noun)| *noun == parts[2]));
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            );
            assert!(SessionName::try_from(name.clone()).is_ok());
            assert!(seen.insert(name));
        }
        assert_eq!(seen.len(), 19_840);
    }

    #[test]
    fn collisions_wrap_and_an_exhausted_pool_fails_without_an_extra_suffix() {
        let first = candidate(0);
        let chosen = available(CAPACITY - 1, |name| Ok(name != first)).unwrap();
        assert_eq!(chosen.as_str(), first);
        assert!(matches!(
            available(0, |_| Ok(true)),
            Err(SessionError::NamesExhausted)
        ));
    }
}
