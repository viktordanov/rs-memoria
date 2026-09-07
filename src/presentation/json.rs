//! The JSON envelope shared by every command.

use memoria_application::error::{Detail, Diagnostic};
use memoria_infrastructure::json;
use memoria_infrastructure::packet::generic_envelope;

pub fn envelope(command: &str, ok: bool, data: &Detail, diagnostics: &[Diagnostic]) -> String {
    // One definition of the envelope: the codec measures refusals against
    // exactly what is serialized here.
    json::to_pretty(&generic_envelope(command, ok, data, diagnostics))
}
