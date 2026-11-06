//! English presentation for shared domain values. Wire identifiers stay unchanged.

use devguard_core::PauseKind;

pub(crate) fn pause_kind(kind: PauseKind) -> &'static str {
    match kind {
        PauseKind::Enforcement => "Enforcement only",
        PauseKind::Service => "Whole service",
    }
}

pub(crate) fn local_record_count(count: usize) -> String {
    format!(
        "{count} local {}",
        if count == 1 { "record" } else { "records" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_uses_natural_english_plurals() {
        assert_eq!(local_record_count(0), "0 local records");
        assert_eq!(local_record_count(1), "1 local record");
        assert_eq!(local_record_count(1000), "1000 local records");
    }

    #[test]
    fn pause_labels_are_english_without_changing_domain_values() {
        assert_eq!(pause_kind(PauseKind::Enforcement), "Enforcement only");
        assert_eq!(pause_kind(PauseKind::Service), "Whole service");
    }
}
