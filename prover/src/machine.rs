use std::marker::PhantomData;

use num_traits::Zero;
use stwo_prover::{
    constraint_framework::TraceLocationAllocator,
    core::{
        air::Component,
        backend::simd::SimdBackend,
        channel::Blake2sChannel,
        fields::qm31::SecureField,
        pcs::{CommitmentSchemeProver, CommitmentSchemeVerifier, PcsConfig},
        poly::circle::{CanonicCoset, PolyOps},
        prover::{prove, verify, ProvingError, StarkProof, VerificationError},
        vcs::blake2_merkle::{Blake2sMerkleChannel, Blake2sMerkleHasher},
    },
};

use super::trace::eval::{
    INTERACTION_TRACE_IDX, ORIGINAL_TRACE_IDX, PREPROCESSED_TRACE_IDX, PROGRAM_TRACE_IDX,
};
use super::trace::{
    program::iter_program_steps, program_trace::ProgramTracesBuilder, sidenote::SideNote,
    PreprocessedTraces, TracesBuilder,
};
use nexus_vm::{
    emulator::{InternalView, MemoryInitializationEntry, ProgramInfo, PublicOutputEntry, View},
    trace::Trace,
};

use super::components::{MachineComponent, MachineEval, LOG_CONSTRAINT_DEGREE};
use super::traits::MachineChip;
use crate::{
    chips::{
        AddChip, AuipcChip, BeqChip, BgeChip, BgeuChip, BitOpChip, BltChip, BltuChip, BneChip,
        CpuChip, DecodingCheckChip, JalChip, JalrChip, LoadStoreChip, LuiChip, ProgramMemCheckChip,
        RangeCheckChip, RegisterMemCheckChip, SllChip, SltChip, SltuChip, SraChip, SrlChip,
        SubChip, SyscallChip, TimestampChip,
    },
    components::AllLookupElements,
    traits::generate_interaction_trace,
};
use serde::{Deserialize, Serialize};
/// Base components tuple for constraining virtual machine execution based on RV32I ISA.
pub type BaseComponents = (
    CpuChip,
    DecodingCheckChip,
    AddChip,
    SubChip,
    SltuChip,
    BitOpChip,
    SltChip,
    BneChip,
    BeqChip,
    BltuChip,
    BltChip,
    BgeuChip,
    BgeChip,
    JalChip,
    LuiChip,
    AuipcChip,
    JalrChip,
    SllChip,
    SrlChip,
    SraChip,
    LoadStoreChip,
    SyscallChip,
    ProgramMemCheckChip,
    RegisterMemCheckChip,
    TimestampChip,
    // Range checks must be positioned at the end. They use values filled by instruction chips.
    RangeCheckChip,
);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub stark_proof: StarkProof<Blake2sMerkleHasher>,
    pub claimed_sum: SecureField,
    pub log_size: u32,
}

/// Main (empty) struct implementing proving functionality of zkVM.
///
/// The generic parameter determines which components are enabled. The default is [`BaseComponents`] for RV32I ISA.
/// This functionality mainly exists for testing and removing a component **does not** remove columns it uses in the AIR.
///
/// Note that the order of components affects correctness, e.g. if columns used by a component require additional lookups,
/// then it should be positioned in the front.
pub struct Machine<C = BaseComponents> {
    _phantom_data: PhantomData<C>,
}

impl<C: MachineChip + Sync> Machine<C> {
    pub fn prove(trace: &impl Trace, view: &View) -> Result<Proof, ProvingError> {
        let num_steps = trace.get_num_steps();
        let program_len = view.get_program_memory().program.len();
        let memory_len = view.get_initial_memory().len()
            + view.get_exit_code().len()
            + view.get_public_output().len();

        let log_size = Self::max_log_size(&[num_steps, program_len, memory_len])
            .max(PreprocessedTraces::MIN_LOG_SIZE);

        let config = PcsConfig::default();
        // Precompute twiddles.
        let twiddles = SimdBackend::precompute_twiddles(
            CanonicCoset::new(
                log_size + LOG_CONSTRAINT_DEGREE + config.fri_config.log_blowup_factor,
            )
            .circle_domain()
            .half_coset,
        );

        // Setup protocol.
        let prover_channel = &mut Blake2sChannel::default();
        let mut commitment_scheme =
            CommitmentSchemeProver::<SimdBackend, Blake2sMerkleChannel>::new(config, &twiddles);

        // Fill columns of the preprocessed trace.
        let preprocessed_trace = PreprocessedTraces::new(log_size);

        // Fill columns of the original trace.
        let mut prover_traces = TracesBuilder::new(log_size);
        let program_traces = ProgramTracesBuilder::new(
            log_size,
            view.get_program_memory(),
            view.get_initial_memory(),
            view.get_exit_code(),
            view.get_public_output(),
        );
        let mut prover_side_note = SideNote::new(&program_traces, view);
        let program_steps = iter_program_steps(trace, prover_traces.num_rows());
        for (row_idx, program_step) in program_steps.enumerate() {
            C::fill_main_trace(
                &mut prover_traces,
                row_idx,
                &program_step,
                &mut prover_side_note,
            );
        }

        let finalized_trace = prover_traces.finalize();
        let finalized_program_trace = program_traces.finalize();

        let mut tree_builder = commitment_scheme.tree_builder();
        let _preprocessed_trace_location =
            tree_builder.extend_evals(preprocessed_trace.clone().into_circle_evaluation());
        tree_builder.commit(prover_channel);

        let mut tree_builder = commitment_scheme.tree_builder();
        let _main_trace_location =
            tree_builder.extend_evals(finalized_trace.clone().into_circle_evaluation());
        tree_builder.commit(prover_channel);

        let mut lookup_elements = AllLookupElements::default();
        C::draw_lookup_elements(&mut lookup_elements, prover_channel);

        let (interaction_trace, claimed_sum) = generate_interaction_trace::<C>(
            &finalized_trace,
            &preprocessed_trace,
            &finalized_program_trace,
            &lookup_elements,
        );

        let mut tree_builder = commitment_scheme.tree_builder();
        let _interaction_trace_location = tree_builder.extend_evals(interaction_trace);
        tree_builder.commit(prover_channel);

        // Fill columns of the program trace.
        let mut tree_builder = commitment_scheme.tree_builder();
        let _program_trace_location =
            tree_builder.extend_evals(finalized_program_trace.into_circle_evaluation());
        tree_builder.commit(prover_channel);

        let component = MachineComponent::new(
            &mut TraceLocationAllocator::default(),
            MachineEval::<C>::new(log_size, lookup_elements),
            claimed_sum,
        );
        let proof = prove::<SimdBackend, Blake2sMerkleChannel>(
            &[&component],
            prover_channel,
            commitment_scheme,
        )?;

        Ok(Proof {
            stark_proof: proof,
            claimed_sum,
            log_size,
        })
    }

    pub fn verify(
        proof: Proof,
        program_info: &ProgramInfo,
        init_memory: &[MemoryInitializationEntry],
        exit_code: &[PublicOutputEntry],
        output_memory: &[PublicOutputEntry],
    ) -> Result<(), VerificationError> {
        let Proof {
            stark_proof: proof,
            claimed_sum,
            log_size,
        } = proof;

        if claimed_sum != SecureField::zero() {
            return Err(VerificationError::InvalidStructure(
                "claimed logup sum is not zero".to_string(),
            ));
        }

        let config = PcsConfig::default();
        let verifier_channel = &mut Blake2sChannel::default();
        let commitment_scheme = &mut CommitmentSchemeVerifier::<Blake2sMerkleChannel>::new(config);

        // simulate the prover and compute expected commitment to preprocessed and program traces
        {
            let config = PcsConfig::default();
            let verifier_channel = &mut Blake2sChannel::default();
            let twiddles = SimdBackend::precompute_twiddles(
                CanonicCoset::new(
                    log_size + LOG_CONSTRAINT_DEGREE + config.fri_config.log_blowup_factor,
                )
                .circle_domain()
                .half_coset,
            );
            let commitment_scheme =
                &mut CommitmentSchemeProver::<SimdBackend, Blake2sMerkleChannel>::new(
                    config, &twiddles,
                );
            let preprocessed_trace = PreprocessedTraces::new(log_size);
            let mut tree_builder = commitment_scheme.tree_builder();
            let _preprocessed_trace_location =
                tree_builder.extend_evals(preprocessed_trace.into_circle_evaluation());
            tree_builder.commit(verifier_channel);

            let preprocessed_expected = commitment_scheme.roots()[PREPROCESSED_TRACE_IDX];
            let preprocessed = proof.commitments[PREPROCESSED_TRACE_IDX];
            if preprocessed_expected != preprocessed {
                return Err(VerificationError::InvalidStructure(format!("invalid commitment to preprocessed trace: \
                                                                        expected {preprocessed_expected}, got {preprocessed}")));
            }

            let program_trace = ProgramTracesBuilder::new(
                log_size,
                program_info,
                init_memory,
                exit_code,
                output_memory,
            )
            .finalize();
            let mut tree_builder = commitment_scheme.tree_builder();
            tree_builder.extend_evals(program_trace.into_circle_evaluation());
            tree_builder.commit(verifier_channel);
            let program_expected = commitment_scheme.roots()[1];
            let program = proof.commitments[PROGRAM_TRACE_IDX];
            if program_expected != program {
                return Err(VerificationError::InvalidStructure(format!("invalid commitment to program trace: \
                                                                        expected {program_expected}, got {program}")));
            }
        }

        // Retrieve the expected column sizes in each commitment interaction, from the AIR.

        // This dummy component is needed for evaluating info about the circuit. The verifier needs to commit to traces
        // and then draw lookup elements, however the component needs a placeholder there that cannot be replaced without
        // refcell hacks.
        //
        // The prover cannot send the component or lookup elements in advance either, because these types have private fields
        // and don't implement serialize.
        let lookup_elements = {
            let dummy_channel = &mut Blake2sChannel::default();
            let mut lookup_elements = AllLookupElements::default();
            C::draw_lookup_elements(&mut lookup_elements, dummy_channel);
            lookup_elements
        };
        let dummy_component = MachineComponent::new(
            &mut TraceLocationAllocator::default(),
            MachineEval::<C>::new(log_size, lookup_elements),
            claimed_sum,
        );
        let sizes = dummy_component.trace_log_degree_bounds();
        for idx in [PREPROCESSED_TRACE_IDX, ORIGINAL_TRACE_IDX] {
            commitment_scheme.commit(proof.commitments[idx], &sizes[idx], verifier_channel);
        }

        let mut lookup_elements = AllLookupElements::default();
        C::draw_lookup_elements(&mut lookup_elements, verifier_channel);
        let component = MachineComponent::new(
            &mut TraceLocationAllocator::default(),
            MachineEval::<C>::new(log_size, lookup_elements),
            claimed_sum,
        );
        // TODO: prover must commit to the program trace before generating challenges.
        for idx in [INTERACTION_TRACE_IDX, PROGRAM_TRACE_IDX] {
            commitment_scheme.commit(proof.commitments[idx], &sizes[idx], verifier_channel);
        }

        verify(&[&component], verifier_channel, commitment_scheme, proof)
    }

    /// Computes minimum allowed log_size from a slice of lengths.
    fn max_log_size(sizes: &[usize]) -> u32 {
        sizes
            .iter()
            .map(|size| size.next_power_of_two().trailing_zeros())
            .max()
            .expect("sizes is empty")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_vm::{
        riscv::{BasicBlock, BuiltinOpcode, Instruction, Opcode},
        trace::k_trace_direct,
    };

    #[test]
    fn prove_verify() {
        let basic_block = vec![BasicBlock::new(vec![
            Instruction::new_ir(Opcode::from(BuiltinOpcode::ADDI), 1, 0, 1),
            Instruction::new_ir(Opcode::from(BuiltinOpcode::ADD), 2, 1, 0),
            Instruction::new_ir(Opcode::from(BuiltinOpcode::ADD), 3, 2, 1),
            Instruction::new_ir(Opcode::from(BuiltinOpcode::ADD), 4, 3, 2),
            Instruction::new_ir(Opcode::from(BuiltinOpcode::ADD), 5, 4, 3),
            Instruction::new_ir(Opcode::from(BuiltinOpcode::ADD), 6, 5, 4),
        ])];
        let (view, program_trace) =
            k_trace_direct(&basic_block, 1).expect("error generating trace");

        let proof = Machine::<BaseComponents>::prove(&program_trace, &view).unwrap();
        Machine::<BaseComponents>::verify(
            proof,
            view.get_program_memory(),
            view.get_initial_memory(),
            view.get_exit_code(),
            view.get_public_output(),
        )
        .unwrap();
    }
}
