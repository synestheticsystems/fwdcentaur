import json
from pathlib import Path
import unittest
from settings import validate_settings


class SettingsContract(unittest.TestCase):
    def test_shared_conformance(self):
        for case in json.loads(Path(__file__).with_name('settings-cases.json').read_text()):
            with self.subTest(case=case['name']):
                if case.get('invalid'):
                    with self.assertRaises(ValueError):
                        validate_settings(case['input'])
                else:
                    self.assertEqual(validate_settings(case['input']), case['expected'])

    def test_an_enabled_schedule_names_the_coordinates_it_is_missing(self):
        with self.assertRaisesRegex(ValueError, r'^settings\.theme_schedule: enabled requires longitude$'):
            validate_settings({'theme_schedule': {'enabled': True, 'latitude': 51.48}})

    def test_nonfinite_numbers_are_rejected(self):
        for value in (float('nan'), float('inf'), -float('inf')):
            with self.assertRaises(ValueError):
                validate_settings({'theme_schedule': {'latitude': value}})
