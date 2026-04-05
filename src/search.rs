pub struct SearchState {
    pub query: String,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
        }
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn pop(&mut self) {
        self.query.pop();
    }

    pub fn clear(&mut self) {
        self.query.clear();
    }

    pub fn matches(&self, title: &str) -> bool {
        if self.query.is_empty() {
            return true;
        }
        title.to_lowercase().contains(&self.query.to_lowercase())
    }

    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }
}
