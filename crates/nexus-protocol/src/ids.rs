use serde::{Deserialize, Serialize};
use specta::Type;
use uuid::Uuid;

/// Defines a newtype wrapper around `Uuid` for one of the distinct identifier
/// kinds from `docs/CONTRACTE_NEXUS_RO.md` §1: these are never interchangeable
/// free strings.
macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

typed_id!(ProjectId);
typed_id!(TaskId);
typed_id!(RunId);
typed_id!(StepId);
typed_id!(ActionId);
typed_id!(OperationId);
typed_id!(ApprovalId);
typed_id!(ArtifactId);
typed_id!(EventId);
typed_id!(RequestId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_ids_are_not_equal() {
        let a = RequestId::new();
        let b = RequestId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn id_round_trips_through_json() {
        let id = TaskId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: TaskId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
