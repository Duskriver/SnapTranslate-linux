/// The request's admission state. Queued requests have not acquired a network slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranslationPhase {
    Queued,
    #[default]
    Running,
}
