//! Room controls retain authored numerical delays/feedback. The SPU has a
//! different filter graph; modulation, low-pass and stereo parity remain open.

pub const WORK_BYTES: u32 = 64 * 1024;
pub const SPU_BYTES: u32 = 512 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// Delays in tenths of a millisecond; feedback in Q15.
    pub reflection: u16,
    pub reflection_feedback: u16,
    pub echo: u16,
    pub echo_feedback: u16,
    pub left_delay: u16,
    /// Original low-pass, modulation, reflection LP, echo LP flags.
    pub filters: u8,
}
pub const PRESETS: [Preset; 29] = [
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 0,
        echo_feedback: 0,
        left_delay: 0,
        filters: 12,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 650,
        echo_feedback: 3277,
        left_delay: 100,
        filters: 4,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 200,
        echo_feedback: 24576,
        left_delay: 100,
        filters: 4,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 300,
        echo_feedback: 25559,
        left_delay: 200,
        filters: 4,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 600,
        echo_feedback: 25231,
        left_delay: 300,
        filters: 4,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 27853,
        echo: 80,
        echo_feedback: 31457,
        left_delay: 100,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 28836,
        echo: 100,
        echo_feedback: 32113,
        left_delay: 200,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 30147,
        echo: 150,
        echo_feedback: 32604,
        left_delay: 400,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 27525,
        echo: 0,
        echo_feedback: 0,
        left_delay: 120,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 29491,
        echo: 0,
        echo_feedback: 0,
        left_delay: 80,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 31130,
        echo: 0,
        echo_feedback: 0,
        left_delay: 40,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 22938,
        echo: 0,
        echo_feedback: 0,
        left_delay: 120,
        filters: 8,
    },
    Preset {
        reflection: 550,
        reflection_feedback: 25559,
        echo: 0,
        echo_feedback: 0,
        left_delay: 80,
        filters: 8,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 28180,
        echo: 0,
        echo_feedback: 0,
        left_delay: 20,
        filters: 8,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 0,
        echo_feedback: 0,
        left_delay: 100,
        filters: 13,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 600,
        echo_feedback: 27853,
        left_delay: 200,
        filters: 13,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 2000,
        echo_feedback: 19661,
        left_delay: 500,
        filters: 13,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 26214,
        echo: 0,
        echo_feedback: 15729,
        left_delay: 160,
        filters: 12,
    },
    Preset {
        reflection: 600,
        reflection_feedback: 29491,
        echo: 0,
        echo_feedback: 17039,
        left_delay: 100,
        filters: 12,
    },
    Preset {
        reflection: 700,
        reflection_feedback: 30802,
        echo: 3000,
        echo_feedback: 19661,
        left_delay: 80,
        filters: 12,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 3000,
        echo_feedback: 13763,
        left_delay: 0,
        filters: 12,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 3500,
        echo_feedback: 15729,
        left_delay: 0,
        filters: 12,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 3800,
        echo_feedback: 19661,
        left_delay: 0,
        filters: 12,
    },
    Preset {
        reflection: 500,
        reflection_feedback: 29491,
        echo: 2000,
        echo_feedback: 9175,
        left_delay: 0,
        filters: 4,
    },
    Preset {
        reflection: 700,
        reflection_feedback: 29491,
        echo: 3000,
        echo_feedback: 13107,
        left_delay: 0,
        filters: 4,
    },
    Preset {
        reflection: 900,
        reflection_feedback: 29491,
        echo: 3500,
        echo_feedback: 16384,
        left_delay: 0,
        filters: 4,
    },
    Preset {
        reflection: 100,
        reflection_feedback: 29491,
        echo: 0,
        echo_feedback: 0,
        left_delay: 500,
        filters: 10,
    },
    Preset {
        reflection: 0,
        reflection_feedback: 0,
        echo: 90,
        echo_feedback: 32735,
        left_delay: 400,
        filters: 12,
    },
    Preset {
        reflection: 10,
        reflection_feedback: 32735,
        echo: 2000,
        echo_feedback: 26214,
        left_delay: 500,
        filters: 8,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Program {
    pub registers: [u16; 32],
    pub wet: u16,
    pub used_bytes: u32,
}
#[optimize(size)]
/// SPU delay offsets address four 22.05 kHz samples per eight-byte word.
fn delay_units(tenths_ms: u32) -> u16 {
    ((tenths_ms * 22_050 + 20_000) / 40_000) as u16
}
// Retail feedback multiplies by256 and truncates before its8-bit shift.
#[optimize(size)]
fn feedback(value: u16) -> u16 {
    value & !127
}
#[optimize(size)]
fn block(cursor: &mut u16, delay: u16) -> (u16, u16) {
    let read = *cursor;
    let write = read + delay;
    *cursor = write + 2;
    (read, write)
}
#[optimize(size)]
pub fn program(room: u8) -> Option<Program> {
    let preset = *PRESETS.get(room as usize)?;
    let mut registers = [0u16; 32];
    if preset.echo == 0 && preset.reflection == 0 {
        return Some(Program {
            registers,
            wet: 0,
            used_bytes: 0,
        });
    }
    let echo = delay_units(preset.echo as u32);
    let reflection = delay_units(preset.reflection as u32);
    let second = delay_units(preset.reflection as u32 * 71 / 100);
    let mut cursor = 2;
    let (left_read, left_write) = block(&mut cursor, echo);
    let (right_read, right_write) = block(&mut cursor, echo);
    let (_, left_first) = block(&mut cursor, reflection);
    let (_, right_first) = block(&mut cursor, reflection);
    let (_, left_second) = block(&mut cursor, second);
    let (_, right_second) = block(&mut cursor, second);
    let unused = cursor;
    cursor += 2;
    let used_bytes = cursor as u32 * 8;
    if used_bytes > WORK_BYTES {
        return None;
    }
    // Echo delay/feedback are carried by the same-side IIR loops. The two
    // reflection times/feedback use the device's all-pass stages; this is
    // audible room control, not original variable-delay comb-filter parity.
    registers[0] = reflection;
    registers[1] = second;
    registers[2] = if preset.filters & 8 != 0 {
        16_384
    } else {
        32_767
    };
    registers[3] = if echo != 0 {
        feedback(preset.echo_feedback)
    } else {
        32_767
    };
    registers[7] = if echo != 0 {
        feedback(preset.echo_feedback)
    } else {
        0
    };
    registers[8] = if reflection != 0 {
        feedback(preset.reflection_feedback)
    } else {
        0
    };
    registers[9] = registers[8];
    registers[10] = left_write;
    registers[11] = right_write;
    registers[12] = if echo != 0 { left_read } else { left_write };
    registers[13] = if echo != 0 { right_read } else { right_write };
    registers[16] = left_read;
    registers[17] = right_read;
    // Cross-side IIR writes get an isolated unobserved word. Disabled comb
    // taps cannot alias or overwrite either active echo/reflection buffer.
    registers[18] = unused;
    registers[19] = unused;
    registers[24] = unused;
    registers[25] = unused;
    registers[26] = left_first;
    registers[27] = right_first;
    registers[28] = left_second;
    registers[29] = right_second;
    registers[30] = 32_767;
    registers[31] = 32_767;
    let wet = if reflection != 0 {
        (11u32 * 32_768 / 32) as u16
    } else {
        32_767
    };
    Some(Program {
        registers,
        wet,
        used_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[optimize(size)]
    #[test]
    fn original_numerical_controls_survive_without_named_preset_substitution() {
        assert_eq!((PRESETS[19].reflection, PRESETS[19].echo), (700, 3000));
        assert_eq!(
            (PRESETS[19].reflection_feedback, PRESETS[19].echo_feedback),
            (30_802, 19_661)
        );
        let p = program(19).unwrap();
        assert_eq!(p.registers[7], 19_584);
        assert_eq!(p.registers[8], 30_720);
        assert_eq!(p.registers[0], 386);
        assert_eq!(p.registers[1], 274);
        assert_eq!(p.wet, 11_264);
    }
    #[optimize(size)]
    #[test]
    fn all_original_presets_have_disjoint_bounded_active_buffers() {
        for room in 1..29 {
            let p = program(room).unwrap();
            if p.wet == 0 {
                assert_eq!(room, 14);
                continue;
            }
            assert!(p.used_bytes <= WORK_BYTES);
            assert!(p.registers[10] < p.registers[11]);
            assert!(p.registers[11] < p.registers[26]);
            assert!(p.registers[26] < p.registers[27]);
            assert!(p.registers[27] < p.registers[28]);
            assert!(p.registers[28] < p.registers[29]);
            assert!(p.registers[29] < p.registers[18]);
            for index in [10, 11, 12, 13, 16, 17, 18, 19, 24, 25, 26, 27, 28, 29] {
                assert!(p.registers[index] as u32 * 8 < WORK_BYTES);
            }
        }
        assert_eq!(program(0).unwrap().wet, 0);
        assert!(program(29).is_none());
    }
    #[optimize(size)]
    #[test]
    fn delay_quantization_is_bounded_by_half_a_hardware_word() {
        for value in 0..=3800u32 {
            let units = delay_units(value) as i64;
            assert!((units * 40_000 - value as i64 * 22_050).abs() <= 20_000);
        }
    }
}

/// Validate the complete bank's upload extent before any DMA. Payload decoding
/// remains the SDK's responsibility; its aligned ADPCM plus parking-tail layout
/// must stay below the independent room workspace.
/// Exact sample admission is independent of optional effect workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub base: u32,
    pub bytes: u32,
}
#[optimize(size)]
pub fn workspace(program: &Program, sample_end: u32) -> Option<Workspace> {
    if program.wet == 0
        || program.used_bytes == 0
        || program.used_bytes > WORK_BYTES
        || sample_end > SPU_BYTES
        || sample_end & 15 != 0
    {
        return None;
    }
    // Every absolute SPU tap (including disabled taps, which the hardware
    // still reads) and each delayed all-pass read stays in the reserved ring.
    if program.registers[10..30]
        .iter()
        .any(|tap| *tap as u32 * 8 + 2 > program.used_bytes)
        || program.registers[26..28]
            .iter()
            .any(|tap| *tap < program.registers[0])
        || program.registers[28..30]
            .iter()
            .any(|tap| *tap < program.registers[1])
    {
        return None;
    }
    let base = SPU_BYTES.checked_sub(program.used_bytes)?;
    if base < sample_end || base & 15 != 0 {
        return None;
    }
    Some(Workspace {
        base,
        bytes: program.used_bytes,
    })
}

#[optimize(size)]
pub fn bank_end(pack: &[u8], base: u32, limit: usize) -> Option<u32> {
    #[optimize(size)]
    fn word(bytes: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    }
    if base > SPU_BYTES || base & 15 != 0 || pack.get(..4)? != b"HSFX" {
        return None;
    }
    let count = word(pack, 4)? as usize;
    if count > limit || 8usize.checked_add(count.checked_mul(8)?)? > pack.len() {
        return None;
    }
    let mut end = base;
    for index in 0..count {
        let offset = word(pack, 8 + index * 8)? as usize;
        let length = word(pack, 12 + index * 8)? as usize;
        let sample = pack.get(offset..offset.checked_add(length)?)?;
        if sample.get(..4)? != b"PSAU" || sample.len() < 32 {
            return None;
        }
        let blocks = word(sample, 24)?.checked_mul(16)?;
        if 32usize.checked_add(blocks as usize)? != sample.len() {
            return None;
        }
        end = end.checked_add(blocks)?.checked_add(15)? & !15;
        end = end.checked_add(16)?;
        if end > SPU_BYTES {
            return None;
        }
    }
    Some(end)
}

#[cfg(test)]
mod bank_tests {
    use super::*;
    #[optimize(size)]
    fn one_sample() -> [u8; 64] {
        let mut bank = [0u8; 64];
        bank[..4].copy_from_slice(b"HSFX");
        bank[4..8].copy_from_slice(&1u32.to_le_bytes());
        bank[8..12].copy_from_slice(&16u32.to_le_bytes());
        bank[12..16].copy_from_slice(&48u32.to_le_bytes());
        bank[16..20].copy_from_slice(b"PSAU");
        bank[40..44].copy_from_slice(&1u32.to_le_bytes());
        bank
    }
    #[optimize(size)]
    #[test]
    fn extent_matches_sdk_adpcm_alignment_and_parking_tail() {
        assert_eq!(bank_end(&one_sample(), 0x1010, 70), Some(0x1030));
        assert_eq!(bank_end(&one_sample(), SPU_BYTES - 32, 70), Some(SPU_BYTES));
        assert_eq!(bank_end(&one_sample(), SPU_BYTES - 16, 70), None);
    }
    #[optimize(size)]
    #[test]
    fn optional_workspace_never_rejects_or_overwrites_valid_samples() {
        assert_eq!(
            bank_end(&one_sample(), SPU_BYTES - WORK_BYTES, 70),
            Some(SPU_BYTES - WORK_BYTES + 32)
        );
        let program = program(19).unwrap();
        let base = SPU_BYTES - program.used_bytes;
        assert_eq!(
            workspace(&program, base),
            Some(Workspace {
                base,
                bytes: program.used_bytes
            })
        );
        assert_eq!(workspace(&program, base + 16), None);
        assert_eq!(workspace(&super::program(0).unwrap(), SPU_BYTES), None);
        let mut invalid = program;
        invalid.registers[19] = (invalid.used_bytes / 8) as u16;
        assert_eq!(workspace(&invalid, 0x1010), None);
        invalid = program;
        invalid.registers[0] = invalid.registers[26] + 1;
        assert_eq!(workspace(&invalid, 0x1010), None);
    }
    #[optimize(size)]
    #[test]
    fn malformed_or_oversized_banks_fail_before_any_upload() {
        let mut b = one_sample();
        assert_eq!(bank_end(&b, 0x1010, 0), None);
        assert_eq!(bank_end(&b[..63], 0x1010, 70), None);
        b[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(bank_end(&b, 0x1010, 70), None);
        b = one_sample();
        b[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(bank_end(&b, 0x1010, 70), None);
    }
}

#[cfg(test)]
mod retail_bank_tests {
    use super::*;
    #[optimize(size)]
    #[test]
    #[ignore = "requires complete cooked103-map core AND dialogue banks"]
    fn all103_actual_combined_banks_preserve_samples_and_admit_only_safe_effects() {
        let root = std::path::PathBuf::from(
            std::env::var("HL_ENV_SOUND_COOKED_DATA").expect("explicit cooked data path"),
        );
        let table = std::fs::read(root.join("sfx/chunk_3052.psxa")).unwrap();
        assert_eq!(&table[..4], b"HCPT");
        let count = u16::from_le_bytes(table[4..6].try_into().unwrap()) as usize;
        assert_eq!(count, 103);
        let mut capacity_limited = Vec::new();
        for map in 0..count {
            let profile = u16::from_le_bytes(table[8 + map * 2..10 + map * 2].try_into().unwrap());
            let profile = if profile == 0 { 3000 } else { profile };
            let core = std::fs::read(root.join(format!("sfx/chunk_{profile}.psxa"))).unwrap();
            let core_end = bank_end(&core, 0x1010, 70).expect("complete core bank");
            // Missing dialogue is a failed fixture, not an absent allocation.
            let voice =
                std::fs::read(root.join(format!("voices/chunk_{}.psxa", 3100 + map))).unwrap();
            let end = bank_end(&voice, core_end, 96).expect("complete dialogue bank");
            assert!(end <= SPU_BYTES, "map{map}");
            if map == 5 {
                assert_eq!(end, 514_752);
                assert!(end > SPU_BYTES - WORK_BYTES);
            }
            let room = std::fs::read(root.join(format!("rooms/room_{}.psxc", map * 2))).unwrap();
            assert!(matches!(&room[..4], b"HLMJ" | b"HLMK"));
            let logic = u32::from_le_bytes(room[48..52].try_into().unwrap()) as usize;
            let records = u16::from_le_bytes(room[logic..logic + 2].try_into().unwrap()) as usize;
            let mut limited = false;
            for record in 0..records {
                let at = logic + 8 + record * 64;
                if room[at] != 63 {
                    continue;
                }
                let preset = u16::from_le_bytes(room[at + 24..at + 26].try_into().unwrap()) as u8;
                let program = program(preset).expect("original room control");
                if program.wet == 0 {
                    continue;
                }
                if let Some(work) = workspace(&program, end) {
                    assert!(work.base >= end && work.base + work.bytes == SPU_BYTES);
                } else {
                    limited = true;
                }
            }
            if limited {
                capacity_limited.push(map);
            }
            if map == 7 {
                assert!(workspace(&program(19).unwrap(), end).is_some());
            }
            if map == 8 {
                assert!(workspace(&program(9).unwrap(), end).is_none());
            }
        }
        assert_eq!(capacity_limited.len(), 45);
        println!("all103 combined banks admitted; neutral-capacity maps: {capacity_limited:?}");
    }
}
