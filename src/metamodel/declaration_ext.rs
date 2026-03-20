use super::concerto_metamodel_1_0_0::{Declaration, Property};

impl Declaration {
    /// Extracts properties from the flattened extra fields.
    /// Returns an empty Vec if no properties are present (e.g., for EnumDeclaration, MapDeclaration).
    pub fn get_properties(&self) -> Vec<Property> {
        if let Some(props_val) = self.extra.get("properties") {
            serde_json::from_value::<Vec<Property>>(props_val.clone()).unwrap_or_default()
        } else {
            Vec::new()
        }
    }
}
