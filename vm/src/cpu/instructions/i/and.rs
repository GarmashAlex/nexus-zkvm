use crate::cpu::instructions::macros::implement_arithmetic_executor;
use crate::{
    cpu::state::{InstructionExecutor, InstructionState},
    memory::{LoadOps, MemoryProcessor, StoreOps},
    riscv::{Instruction, InstructionType, Register},
};
use nexus_common::cpu::{Processor, Registers};

pub struct AndInstruction {
    rd: (Register, u32),
    rs1: u32,
    rs2: u32,
}

implement_arithmetic_executor!(AndInstruction, |a: u32, b: u32| a & b);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::state::Cpu;
    use crate::riscv::{BuiltinOpcode, Instruction, Opcode, Register};

    #[test]
    fn test_and_instruction() {
        let mut cpu = Cpu::default();

        // Set initial register values
        cpu.registers.write(Register::X1, 0b1010);
        cpu.registers.write(Register::X2, 0b1100);

        let bare_instruction = Instruction::new_ir(Opcode::from(BuiltinOpcode::AND), 3, 1, 2);

        let mut instruction = AndInstruction::decode(&bare_instruction, &cpu.registers);

        // Execute the and instruction
        instruction.execute();
        let res = instruction.write_back(&mut cpu);

        // Check the result (1010 & 1100 = 1000)
        assert_eq!(res, Some(0b1000));
        assert_eq!(cpu.registers.read(Register::X3), 0b1000);
    }

    #[test]
    fn test_and_with_zero() {
        let mut cpu = Cpu::default();

        // Set initial register values
        cpu.registers.write(Register::X1, 0xFFFFFFFF);
        cpu.registers.write(Register::X2, 0);

        let bare_instruction = Instruction::new_ir(Opcode::from(BuiltinOpcode::AND), 3, 1, 2);

        let mut instruction = AndInstruction::decode(&bare_instruction, &cpu.registers);

        // Execute the and instruction
        instruction.execute();
        let res = instruction.write_back(&mut cpu);

        // Check the result (anything AND 0 should be 0)
        assert_eq!(res, Some(0));
        assert_eq!(cpu.registers.read(Register::X3), 0);
    }

    #[test]
    fn test_and_with_all_ones() {
        let mut cpu = Cpu::default();

        // Set initial register values
        cpu.registers.write(Register::X1, 0xABCDEF12);
        cpu.registers.write(Register::X2, 0xFFFFFFFF);

        let bare_instruction = Instruction::new_ir(Opcode::from(BuiltinOpcode::AND), 3, 1, 2);

        let mut instruction = AndInstruction::decode(&bare_instruction, &cpu.registers);

        // Execute the and instruction
        instruction.execute();
        let res = instruction.write_back(&mut cpu);

        // Check the result (anything AND all 1's should be itself)
        assert_eq!(res, Some(0xABCDEF12));
        assert_eq!(cpu.registers.read(Register::X3), 0xABCDEF12);
    }

    #[test]
    fn test_and_same_register() {
        let mut cpu = Cpu::default();

        // Set initial register value
        cpu.registers.write(Register::X1, 0xAA55AA55);

        let bare_instruction = Instruction::new_ir(Opcode::from(BuiltinOpcode::AND), 1, 1, 1);

        let mut instruction = AndInstruction::decode(&bare_instruction, &cpu.registers);

        // Execute the and instruction
        instruction.execute();
        let res = instruction.write_back(&mut cpu);

        // Check the result (AND with itself should be itself)
        assert_eq!(res, Some(0xAA55AA55));
        assert_eq!(cpu.registers.read(Register::X1), 0xAA55AA55);
    }
}
