#!/usr/bin/env python3
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import roster_audit as audit


class CookedPropTypeTests(unittest.TestCase):
    def test_runtime_type_mask_ignores_all_prop_metadata_bits(self) -> None:
        self.assertEqual(audit.cooked_prop_type(0x0000), 0)
        self.assertEqual(audit.cooked_prop_type(0x2000), 0)  # predisaster scientist
        self.assertEqual(audit.cooked_prop_type(0x4002), 2)  # dormant headcrab
        self.assertEqual(audit.cooked_prop_type(0x8008), 8)  # dead hgrunt


if __name__ == "__main__":
    unittest.main()
