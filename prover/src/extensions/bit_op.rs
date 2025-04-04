use stwo_prover::{
    constraint_framework::{
        logup::LogupTraceGenerator, preprocessed_columns::PreProcessedColumnId, FrameworkEval,
        Relation, RelationEntry,
    },
    core::{
        backend::simd::{column::BaseColumn, m31::LOG_N_LANES, SimdBackend},
        fields::{m31::BaseField, qm31::SecureField},
        poly::{
            circle::{CanonicCoset, CircleEvaluation},
            BitReversedOrder,
        },
        ColumnVec,
    },
};

use crate::{
    chips::instructions::bit_op::{BitOp, BitOpLookupElements},
    components::AllLookupElements,
    trace::sidenote::SideNote,
};

use super::{BuiltInExtension, FrameworkEvalExt};

/// A component that yields logup sum emitted by the bitwise chip.
#[derive(Debug, Clone)]
pub struct BitOpMultiplicity {
    _private: (),
}

impl BitOpMultiplicity {
    pub(super) const fn new() -> Self {
        Self { _private: () }
    }
}

pub(crate) struct BitOpMultiplicityEval {
    lookup_elements: BitOpLookupElements,
}

impl Default for BitOpMultiplicityEval {
    fn default() -> Self {
        Self {
            lookup_elements: BitOpLookupElements::dummy(),
        }
    }
}

impl BitOpMultiplicityEval {
    // There are (2 ** 4) ** 2 = 256 combinations for each looked up pair.
    const LOG_SIZE: u32 = 8;
}

impl FrameworkEval for BitOpMultiplicityEval {
    fn log_size(&self) -> u32 {
        Self::LOG_SIZE
    }

    fn max_constraint_log_degree_bound(&self) -> u32 {
        Self::LOG_SIZE + 1
    }

    fn evaluate<E: stwo_prover::constraint_framework::EvalAtRow>(&self, mut eval: E) -> E {
        const PREPROCESSED_COL_IDS: &[&str] = &[
            "preprocessed_bitwise_input_b",
            "preprocessed_bitwise_input_c",
            "preprocessed_bitwise_output_and",
            "preprocessed_bitwise_output_or",
            "preprocessed_bitwise_output_xor",
        ];
        let preprocessed_columns: Vec<E::F> = PREPROCESSED_COL_IDS
            .iter()
            .map(|&id| eval.get_preprocessed_column(PreProcessedColumnId { id: id.to_owned() }))
            .collect();

        let [answer_b, answer_c, answer_a_and, answer_a_or, answer_a_xor] = preprocessed_columns
            .try_into()
            .expect("invalid number of preprocessed columns");

        let mult_and = eval.next_trace_mask();
        let mult_or = eval.next_trace_mask();
        let mult_xor = eval.next_trace_mask();

        // Subtract looked up multiplicities from logup sum
        for (op_type, answer_a, mult) in [
            (BitOp::And, answer_a_and, mult_and),
            (BitOp::Or, answer_a_or, mult_or),
            (BitOp::Xor, answer_a_xor, mult_xor),
        ] {
            let op_type = E::F::from(op_type.to_base_field());
            let numerator: E::EF = (-mult).into();
            eval.add_to_relation(RelationEntry::new(
                &self.lookup_elements,
                numerator,
                &[op_type, answer_b.clone(), answer_c.clone(), answer_a],
            ));
        }

        eval.finalize_logup();
        eval
    }
}

impl FrameworkEvalExt for BitOpMultiplicityEval {
    const LOG_SIZE: u32 = BitOpMultiplicityEval::LOG_SIZE;

    fn new(lookup_elements: &AllLookupElements) -> Self {
        let lookup_elements: &BitOpLookupElements = lookup_elements.as_ref();
        Self {
            lookup_elements: lookup_elements.clone(),
        }
    }
}

impl BuiltInExtension for BitOpMultiplicity {
    type Eval = BitOpMultiplicityEval;

    fn generate_preprocessed_trace(
    ) -> ColumnVec<CircleEvaluation<SimdBackend, BaseField, BitReversedOrder>> {
        let base_cols = Self::preprocessed_base_columns();
        let domain = CanonicCoset::new(BitOpMultiplicityEval::LOG_SIZE).circle_domain();
        base_cols
            .into_iter()
            .map(|col| CircleEvaluation::new(domain, col))
            .collect()
    }

    fn preprocessed_trace_sizes() -> Vec<u32> {
        // preprocessed column for each of [and, or, xor] with 2 input lookups
        std::iter::repeat(BitOpMultiplicityEval::LOG_SIZE)
            .take(5)
            .collect()
    }

    /// Contains multiplicity column for each of [and, or, xor]
    ///
    /// The ordering of rows is the same as the ordering of the preprocessed value column.
    fn generate_original_trace(
        side_note: &SideNote,
    ) -> ColumnVec<CircleEvaluation<SimdBackend, BaseField, BitReversedOrder>> {
        let base_cols = Self::base_columns(side_note);
        let domain = CanonicCoset::new(BitOpMultiplicityEval::LOG_SIZE).circle_domain();
        base_cols
            .into_iter()
            .map(|col| CircleEvaluation::new(domain, col))
            .collect()
    }

    fn generate_interaction_trace(
        side_note: &SideNote,
        lookup_elements: &AllLookupElements,
    ) -> (
        ColumnVec<CircleEvaluation<SimdBackend, BaseField, BitReversedOrder>>,
        SecureField,
    ) {
        let lookup_element: &BitOpLookupElements = lookup_elements.as_ref();
        let mut logup_trace_gen = LogupTraceGenerator::new(BitOpMultiplicityEval::LOG_SIZE);

        // Subtract looked up multiplicities from logup sum
        let preprocessed_columns = Self::preprocessed_base_columns();
        let base_columns = Self::base_columns(side_note);

        let [answer_b, answer_c, answer_a_and, answer_a_or, answer_a_xor] = preprocessed_columns
            .try_into()
            .expect("invalid number of preprocessed columns");
        let [mult_and, mult_or, mult_xor] = base_columns
            .try_into()
            .expect("invalid number of columns in original trace");

        for (op_type, answer_a, mult) in [
            (BitOp::And, &answer_a_and, &mult_and),
            (BitOp::Or, &answer_a_or, &mult_or),
            (BitOp::Xor, &answer_a_xor, &mult_xor),
        ] {
            let mut logup_col_gen = logup_trace_gen.new_col();
            for vec_row in 0..(1 << (BitOpMultiplicityEval::LOG_SIZE - LOG_N_LANES)) {
                let answer_tuple = vec![
                    op_type.to_packed_base_field(),
                    answer_b.data[vec_row],
                    answer_c.data[vec_row],
                    answer_a.data[vec_row],
                ];
                let denom = lookup_element.combine(&answer_tuple);
                let numerator = mult.data[vec_row];
                logup_col_gen.write_frac(vec_row, (-numerator).into(), denom);
            }
            logup_col_gen.finalize_col();
        }

        logup_trace_gen.finalize_last()
    }
}

impl BitOpMultiplicity {
    fn preprocessed_base_columns() -> Vec<BaseColumn> {
        let range_iter = (0u8..16).flat_map(|b| (0u8..16).map(move |c| (b, c)));
        let column_b = BaseColumn::from_iter(range_iter.clone().map(|(b, _)| u32::from(b).into()));
        let column_c = BaseColumn::from_iter(range_iter.clone().map(|(_, c)| u32::from(c).into()));
        let column_and =
            BaseColumn::from_iter(range_iter.clone().map(|(b, c)| u32::from(b & c).into()));
        let column_or =
            BaseColumn::from_iter(range_iter.clone().map(|(b, c)| u32::from(b | c).into()));
        let column_xor =
            BaseColumn::from_iter(range_iter.clone().map(|(b, c)| u32::from(b ^ c).into()));

        vec![column_b, column_c, column_and, column_or, column_xor]
    }

    fn base_columns(side_note: &SideNote) -> Vec<BaseColumn> {
        let multiplicity_and = &side_note.bit_op.multiplicity_and;
        let multiplicity_or = &side_note.bit_op.multiplicity_or;
        let multiplicity_xor = &side_note.bit_op.multiplicity_xor;

        let multiplicity_and = BaseColumn::from_iter(
            (0..=255).map(|i| multiplicity_and.get(&i).copied().unwrap_or_default().into()),
        );
        let multiplicity_or = BaseColumn::from_iter(
            (0..=255).map(|i| multiplicity_or.get(&i).copied().unwrap_or_default().into()),
        );
        let multiplicity_xor = BaseColumn::from_iter(
            (0..=255).map(|i| multiplicity_xor.get(&i).copied().unwrap_or_default().into()),
        );
        vec![multiplicity_and, multiplicity_or, multiplicity_xor]
    }
}
