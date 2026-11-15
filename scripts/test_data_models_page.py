"""The data models page's script (data-models-page.py) stops at a model its mapping does not place, and
at an entry of its mapping that names no model, naming each. CI's check of the page runs the script on
the models the code declares, all of which the mapping places."""
from pathlib import Path
import importlib.util
import unittest

spec = importlib.util.spec_from_file_location('data_models_page', Path(__file__).with_name('data-models-page.py'))
data_models_page = importlib.util.module_from_spec(spec)
spec.loader.exec_module(data_models_page)


def model(path, label=None):
    """A model as the script reads one from a Rust file, with only a path and a label."""
    return {'name': path.rpartition('::')[2], 'path': path, 'label': label, 'file': 'src/model.rs', 'line': 1,
            'doc': '', 'fields': []}


# A model for each entry of the mapping, and no other.
FWD = [model(path) for path in [*data_models_page.COUNTERPART, *data_models_page.FOR_OLDER_STORES]]
COMMONS = [model(path) for path in [*data_models_page.COMMONS_USED, *data_models_page.COMMONS_PLANNED]]


class Mapping(unittest.TestCase):
    def assert_stops(self, fwd, commons, sentence):
        with self.assertRaisesRegex(data_models_page.PageError, sentence):
            data_models_page.place(fwd, commons)

    def test_a_model_it_does_not_place_stops_the_run_by_name(self):
        with self.subTest('fwd'):
            self.assert_stops(FWD + [model('Ledger')], COMMONS,
                              r'fwd model Ledger \(src/model.rs:1\) is not in the mapping')
        with self.subTest('Commons'):
            self.assert_stops(FWD, COMMONS + [model('Vote', 'intention-graph.vote')],
                              r'Commons model Vote \(src/model.rs:1\) is not in the mapping')

    def test_an_entry_that_names_no_model_stops_the_run_by_name(self):
        with self.subTest('fwd'):
            self.assert_stops(FWD[1:], COMMONS, f'places fwd model {FWD[0]["path"]}, which')
        with self.subTest('Commons'):
            self.assert_stops(FWD, COMMONS[:-1], f'places Commons model {COMMONS[-1]["path"]}, which')


if __name__ == '__main__':
    unittest.main()
