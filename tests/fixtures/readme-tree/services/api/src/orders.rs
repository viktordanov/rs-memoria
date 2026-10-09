use std::collections::HashMap;

pub struct Orders {
    items: HashMap<u64, String>,
}

impl Orders {
    pub fn create(&mut self, id: u64, book: String) {
        self.items.insert(id, book);
    }

    pub fn get(&self, id: u64) -> Option<&String> {
        self.items.get(&id)
    }
}
