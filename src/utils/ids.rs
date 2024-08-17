macro_rules! id_gen {
    ($mod_name:ident) => {
        mod $mod_name {
            use once_cell::sync::Lazy;
            use std::{collections::HashSet, sync::Mutex};

            static ID_DATA: Lazy<Mutex<(HashSet<usize>, usize)>> =
                Lazy::new(|| Mutex::new((HashSet::new(), 0)));

            pub(crate) fn next() -> usize {
                let (id_set, counter) = &mut *ID_DATA.lock().unwrap();

                if id_set.len() == usize::MAX {
                    panic!("Out of ids");
                }

                while !id_set.insert(*counter) {
                    *counter = counter.wrapping_add(1);
                }

                let new_id = *counter;
                *counter = counter.wrapping_add(1);

                new_id
            }

            pub(crate) fn remove(id: usize) -> bool {
                ID_DATA.lock().unwrap().0.remove(&id)
            }
        }
    };
}

pub(crate) use id_gen;
