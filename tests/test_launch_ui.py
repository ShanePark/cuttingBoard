from __future__ import annotations

import inspect
import tempfile
import unittest
from pathlib import Path

from cutting_board.ui import theme
from cutting_board.ui.launch_dialogs import (
    LaunchLogDialog,
    LaunchProfileDialog,
    LaunchProfileDraft,
    LaunchProfileValidationError,
    LaunchTaskDraft,
    validate_profile_draft,
)
from cutting_board.ui.launch_widgets import (
    PROFILE_BORDER_WIDTH,
    PROFILE_INSET,
    TASK_BORDER_WIDTH,
    TASK_INSET,
    ActionButton,
    LaunchProfileView,
    LaunchTaskView,
    ProfileCard,
    _action_insets,
    _middle_ellipsis,
    _profiles_changed,
    profile_primary_action,
    state_presentation,
    task_primary_action,
)


def _draft(root: Path, *tasks: LaunchTaskDraft) -> LaunchProfileDraft:
    return LaunchProfileDraft(" Dutypark ", str(root), tuple(tasks))


def _relative_luminance(colour: str) -> float:
    channels = [int(colour[index : index + 2], 16) / 255 for index in (1, 3, 5)]
    linear = [
        value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4
        for value in channels
    ]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def _contrast_ratio(foreground: str, background: str) -> float:
    lighter, darker = sorted(
        (_relative_luminance(foreground), _relative_luminance(background)),
        reverse=True,
    )
    return (lighter + 0.05) / (darker + 0.05)


class LaunchUiTest(unittest.TestCase):
    def setUp(self) -> None:
        self._temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary_directory.name)

    def tearDown(self) -> None:
        self._temporary_directory.cleanup()

    def test_profile_draft_is_normalized_and_accepts_relative_task_cwd(self) -> None:
        draft = _draft(
            self.root,
            LaunchTaskDraft(
                " Backend ",
                ".",
                " ./gradlew bootRun ",
                8080,
                " ./gradlew classes --continuous ",
            ),
            LaunchTaskDraft("Frontend", "frontend", "npm run dev", 5173),
        )

        result = validate_profile_draft(draft)

        self.assertEqual(result.name, "Dutypark")
        self.assertEqual(result.project_root, str(self.root.resolve()))
        self.assertEqual(
            result.tasks[0],
            LaunchTaskDraft(
                "Backend",
                ".",
                "./gradlew bootRun",
                8080,
                "./gradlew classes --continuous",
            ),
        )

    def test_profile_draft_rejects_invalid_ports(self) -> None:
        for port in (0, 65536, -1, True):
            with self.subTest(port=port), self.assertRaisesRegex(
                LaunchProfileValidationError,
                "1~65535",
            ):
                validate_profile_draft(
                    _draft(self.root, LaunchTaskDraft("Backend", ".", "run", port))
                )

    def test_profile_draft_rejects_case_insensitive_duplicate_task_names(self) -> None:
        with self.assertRaisesRegex(LaunchProfileValidationError, "중복"):
            validate_profile_draft(
                _draft(
                    self.root,
                    LaunchTaskDraft("Backend", ".", "run"),
                    LaunchTaskDraft(" backend ", ".", "run again"),
                )
            )

    def test_profile_draft_rejects_task_cwd_outside_project(self) -> None:
        with self.assertRaisesRegex(LaunchProfileValidationError, "프로젝트 안"):
            validate_profile_draft(
                _draft(
                    self.root / "project",
                    LaunchTaskDraft("Backend", "../outside", "run"),
                )
            )

    def test_external_task_has_explicit_external_presentation(self) -> None:
        task = LaunchTaskView(
            name="Backend",
            cwd=".",
            command="run",
            state="running",
            external=True,
            can_start=False,
            can_stop=False,
        )

        presentation = state_presentation(task.state, external=task.external)

        self.assertEqual(presentation.label, "외부 실행 중")
        self.assertFalse(task.can_stop)

    def test_unknown_state_remains_visible_instead_of_looking_stopped(self) -> None:
        self.assertEqual(state_presentation("checking").label, "checking")

    def test_semantic_launch_colours_clear_contrast_in_both_palettes(self) -> None:
        for mode in ("dark", "light"):
            with self.subTest(mode=mode):
                theme.apply_palette(mode)
                self.assertGreaterEqual(_contrast_ratio(theme.ON_ACCENT, theme.ACCENT), 4.5)
                self.assertGreaterEqual(
                    _contrast_ratio(theme.ON_ACCENT, theme.ACCENT_HOVER),
                    4.5,
                )
                for state in ("stopped", "starting", "running", "failed", "external"):
                    with self.subTest(mode=mode, state=state):
                        presentation = state_presentation(state)
                        self.assertGreaterEqual(
                            _contrast_ratio(presentation.colour, theme.SURFACE_ALT),
                            4.5,
                        )
        theme.apply_palette("dark")

    def test_widget_colour_defaults_are_resolved_when_constructed(self) -> None:
        action_parameters = inspect.signature(ActionButton.__init__).parameters
        self.assertIsNone(action_parameters["foreground"].default)
        self.assertIsNone(action_parameters["background"].default)
        self.assertIsNone(action_parameters["hover"].default)

        card_parameters = inspect.signature(ProfileCard._button).parameters
        self.assertIsNone(card_parameters["colour"].default)

    def test_profile_exposes_only_the_contextual_group_action(self) -> None:
        task = LaunchTaskView("Backend", ".", "run")
        cases = (
            (True, False, ("start", "▶ 전체 실행")),
            (False, True, ("stop", "■ 전체 종료")),
            (True, True, ("start", "▶ 나머지")),
            (False, False, None),
        )
        for can_start, can_stop, expected in cases:
            with self.subTest(can_start=can_start, can_stop=can_stop):
                profile = LaunchProfileView(
                    "dutypark",
                    "Dutypark",
                    str(self.root),
                    (task,),
                    can_start=can_start,
                    can_stop=can_stop,
                )
                action = profile_primary_action(profile)
                actual = None if action is None else (action.key, action.label)
                self.assertEqual(actual, expected)

    def test_task_exposes_stop_before_start_and_never_both(self) -> None:
        cases = (
            (True, False, "start"),
            (False, True, "stop"),
            (True, True, "stop"),
            (False, False, None),
        )
        for can_start, can_stop, expected in cases:
            with self.subTest(can_start=can_start, can_stop=can_stop):
                task = LaunchTaskView(
                    "Backend",
                    ".",
                    "run",
                    can_start=can_start,
                    can_stop=can_stop,
                )
                action = task_primary_action(task)
                self.assertEqual(None if action is None else action.key, expected)

    def test_surfaces_use_one_quiet_hairline_and_consistent_insets(self) -> None:
        self.assertEqual(PROFILE_BORDER_WIDTH, 1)
        self.assertEqual(TASK_BORDER_WIDTH, 1)
        self.assertEqual(PROFILE_INSET, (18, 16))
        self.assertEqual(TASK_INSET, (14, 11))

    def test_compact_actions_reduce_chrome_without_losing_padding(self) -> None:
        compact = _action_insets(compact=True)
        regular = _action_insets(compact=False)

        self.assertLess(compact.horizontal, regular.horizontal)
        self.assertLess(compact.vertical, regular.vertical)
        self.assertGreaterEqual(compact.horizontal, 8)
        self.assertGreaterEqual(compact.vertical, 4)

    def test_launch_dialogs_use_their_explicit_scrim_dismiss_policies(self) -> None:
        log_initializer = inspect.getsource(LaunchLogDialog.__init__)
        form_initializer = inspect.getsource(LaunchProfileDialog.__init__)

        self.assertIn("configure_detail_dismiss(self, backdrop)", log_initializer)
        self.assertNotIn("configure_detail_dismiss", form_initializer)
        self.assertIn("backdrop.activate(self, on_outside=None)", form_initializer)
        self.assertIn('self.bind("<Escape>"', form_initializer)
        self.assertIn('self.protocol("WM_DELETE_WINDOW", self.destroy)', form_initializer)

    def test_long_values_keep_both_identifying_ends(self) -> None:
        value = "/Users/shane/Documents/GitHub/dutypark/frontend/src/really-long-component-name.tsx"

        shortened = _middle_ellipsis(value, 36)

        self.assertEqual(len(shortened), 36)
        self.assertTrue(shortened.startswith("/Users/shane"))
        self.assertTrue(shortened.endswith("component-name.tsx"))
        self.assertIn("…", shortened)

    def test_identical_profile_snapshot_does_not_request_a_rebuild(self) -> None:
        profile = LaunchProfileView(
            "dutypark",
            "Dutypark",
            str(self.root),
            (LaunchTaskView("Backend", ".", "run"),),
        )
        snapshot = (profile,)

        self.assertTrue(_profiles_changed((), snapshot, rendered=False))
        self.assertFalse(_profiles_changed(snapshot, tuple(snapshot), rendered=True))
        changed = (LaunchProfileView("dutypark", "Dutypark", str(self.root), (), can_stop=True),)
        self.assertTrue(_profiles_changed(snapshot, changed, rendered=True))


if __name__ == "__main__":
    unittest.main()
