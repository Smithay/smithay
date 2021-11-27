// TODO: Remove once desktop is back
#![allow(unused)]

macro_rules! id_gen {
    ($func_name:ident, $id_name:ident, $ids_name:ident) => {
        static $id_name: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        lazy_static::lazy_static! {
            static ref $ids_name: std::sync::Mutex<std::collections::HashSet<usize>> =
                std::sync::Mutex::new(std::collections::HashSet::new());
        }

        fn $func_name() -> usize {
            let mut ids = $ids_name.lock().unwrap();
            if ids.len() == usize::MAX {
                panic!("Out of ids");
            }

            let id = loop {
                let new_id = $id_name.fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |mut id| {
                        while ids.iter().any(|k| *k == id) {
                            id += 1;
                        }
                        id += 1;
                        Some(id)
                    },
                );
                if let Ok(id) = new_id {
                    break id;
                }
            };

            ids.insert(id);
            id
        }
    };
}

pub(crate) use id_gen;
