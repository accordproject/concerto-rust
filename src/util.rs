/// Utility functions for Concerto
pub mod util {
    use semver::Version;

    /// Checks if a string is a valid semver version
    pub fn is_valid_semantic_version(version: &str) -> bool {
        Version::parse(version).is_ok()
    }

    /// Compares two semver versions
    pub fn compare_versions(version1: &str, version2: &str) -> Result<std::cmp::Ordering, String> {
        let v1 = Version::parse(version1).map_err(|e| e.to_string())?;
        let v2 = Version::parse(version2).map_err(|e| e.to_string())?;

        Ok(v1.cmp(&v2))
    }
}
