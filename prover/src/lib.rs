pub mod chips;
pub mod components;
pub mod trace;

pub mod column;
pub mod traits;
pub mod virtual_column;

pub mod machine;

#[cfg(test)]
mod test_utils;

use nexus_vm::emulator::InternalView;
pub(crate) use nexus_vm::WORD_SIZE;

pub use machine::Proof;

pub use stwo_prover::core::prover::{ProvingError, VerificationError};

pub fn prove(
    trace: &impl nexus_vm::trace::Trace,
    view: &nexus_vm::emulator::View,
) -> Result<Proof, ProvingError> {
    machine::Machine::<machine::BaseComponents>::prove(trace, view)
}

pub fn verify(proof: Proof, view: &nexus_vm::emulator::View) -> Result<(), VerificationError> {
    machine::Machine::<machine::BaseComponents>::verify(
        proof,
        view.get_program_memory(),
        view.get_initial_memory(),
        view.get_exit_code(),
        view.get_public_output(),
    )
}
