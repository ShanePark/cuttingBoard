from __future__ import annotations

import argparse
import queue
import signal
import tempfile
import unittest
from concurrent.futures import Future
from pathlib import Path
from unittest.mock import Mock, patch

from cutting_board.app import _close_on_signal, _launch_controller_for, _run_gui, build_parser
from cutting_board.launch_models import (
    LaunchProfile,
    LaunchState,
    LaunchTask,
    ManagedTaskSnapshot,
)
from cutting_board.services.settings import UISettings
from cutting_board.ui import theme
from cutting_board.ui.dialogs import THEME_MODE_CHOICES, SettingsDialog
from cutting_board.ui.main_window import (
    HEADER_HEIGHT,
    SETTINGS_HIT_TARGET,
    TAB_LAUNCH,
    CuttingBoardWindow,
    _bind_action,
    _gear_polygon_points,
    _listener_count_label,
    _segmented_surface_colours,
    _SettingsGear,
    _toolbar_surface_colours,
)
from tests.helpers import make_project, make_service, make_snapshot


class _Settings:
    window_geometry = "1200x760"
    theme_mode = "dark"


class _Root:
    def __init__(self, calls: list[str]) -> None:
        self.calls = calls

    def winfo_geometry(self) -> str:
        return "1200x760+0+0"

    def destroy(self) -> None:
        self.calls.append("root")


class _SettingsStore:
    def __init__(self, calls: list[str]) -> None:
        self.calls = calls

    def save(self, settings: object) -> None:
        del settings
        self.calls.append("settings")


class _LaunchController:
    def __init__(
        self,
        calls: list[str],
        snapshots: tuple[ManagedTaskSnapshot, ...] = (),
    ) -> None:
        self.calls = calls
        self._snapshots = snapshots
        self.stop_calls = 0

    def snapshots(self) -> tuple[ManagedTaskSnapshot, ...]:
        return self._snapshots

    def close(self) -> None:
        self.calls.append("launch")

    def snapshot(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        del profile_id, task_name
        return self._snapshots[0]

    def stop_task(self, profile_id: str, task_name: str) -> None:
        del profile_id, task_name
        self.stop_calls += 1


class _ScannerController:
    def __init__(self, calls: list[str]) -> None:
        self.calls = calls

    def close(self) -> None:
        self.calls.append("scanner")


class _Executor:
    def __init__(self, calls: list[str]) -> None:
        self.calls = calls

    def shutdown(self, *, wait: bool, cancel_futures: bool) -> None:
        self.calls.append("executor")
        assert wait is False
        assert cancel_futures is True


class _ImmediateExecutor:
    def submit(self, action: object) -> Future[object]:
        future: Future[object] = Future()
        try:
            assert callable(action)
            future.set_result(action())
        except Exception as exc:  # noqa: BLE001 - mimic Future failure capture
            future.set_exception(exc)
        return future


def _window(
    calls: list[str],
    launch_snapshots: tuple[ManagedTaskSnapshot, ...] = (),
) -> CuttingBoardWindow:
    window = CuttingBoardWindow.__new__(CuttingBoardWindow)
    window._closing = False
    window.root = _Root(calls)
    window.settings = _Settings()
    window.settings_store = _SettingsStore(calls)
    window.launch_controller = _LaunchController(calls, launch_snapshots)
    window.controller = _ScannerController(calls)
    window.executor = _Executor(calls)
    window.fonts = {}
    window.icons = Mock()
    return window


class LaunchAppCompositionTests(unittest.TestCase):
    def test_parser_accepts_isolated_launch_profile_path(self) -> None:
        path = Path("/tmp/cutting-board-test-launch.json")

        args = build_parser().parse_args(["--demo", "--launch-profiles-file", str(path)])

        self.assertEqual(args.launch_profiles_file, path)

    def test_real_composition_passes_explicit_path_to_store(self) -> None:
        path = Path("/tmp/cutting-board-test-launch.json")
        args = argparse.Namespace(demo=False, launch_profiles_file=path)
        store = object()
        controller = object()

        with (
            patch("cutting_board.app.LaunchProfileStore", return_value=store) as store_type,
            patch("cutting_board.app.LaunchController", return_value=controller) as controller_type,
        ):
            result = _launch_controller_for(args)

        self.assertIs(result, controller)
        store_type.assert_called_once_with(path)
        controller_type.assert_called_once_with(store)

    def test_demo_composition_never_constructs_managed_runner(self) -> None:
        args = argparse.Namespace(demo=True, launch_profiles_file=None)

        with patch("cutting_board.app.LaunchController") as controller_type:
            controller = _launch_controller_for(args)
            controller.start_profile("demo-shop")
            controller.stop_profile("demo-shop")
            controller.close()

        controller_type.assert_not_called()
        self.assertTrue(controller.closed)

    def test_saved_palette_is_applied_before_window_construction(self) -> None:
        calls: list[str] = []
        root = Mock()
        settings = UISettings(theme_mode="light")
        store = Mock()
        store.load.return_value = settings
        args = argparse.Namespace(
            settings_file=None,
            scan_interval=None,
            demo=True,
            auto_close=None,
            launch_profiles_file=None,
        )

        with (
            patch("tkinter.Tk", return_value=root),
            patch("cutting_board.app.SettingsStore", return_value=store),
            patch("cutting_board.app._launch_controller_for", return_value=Mock()),
            patch("cutting_board.app._close_on_signal"),
            patch(
                "cutting_board.ui.theme.apply_palette",
                side_effect=lambda mode: calls.append(f"palette:{mode}"),
            ),
            patch(
                "cutting_board.ui.main_window.CuttingBoardWindow",
                side_effect=lambda *args, **kwargs: calls.append("window") or Mock(),
            ),
        ):
            result = _run_gui(args, Mock())

        self.assertEqual(result, 0)
        self.assertEqual(calls, ["palette:light", "window"])


class LaunchCloseLifecycleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.active = ManagedTaskSnapshot(
            profile_id="dutypark",
            task_name="Backend",
            state=LaunchState.RUNNING,
            main_pid=1234,
        )

    def test_interactive_close_notifies_then_closes_launch_before_scanner_and_root(self) -> None:
        calls: list[str] = []
        window = _window(calls, (self.active,))

        with patch("cutting_board.ui.main_window.ask_confirmation", return_value=True) as ask:
            window.close()

        self.assertEqual(calls, ["settings", "launch", "scanner", "executor", "root"])
        self.assertEqual(
            ask.call_args.kwargs["question"],
            "All processes started by Cutting Board will be stopped.",
        )

    def test_cancelled_interactive_close_keeps_everything_running(self) -> None:
        calls: list[str] = []
        window = _window(calls, (self.active,))

        with patch("cutting_board.ui.main_window.ask_confirmation", return_value=False):
            window.close()

        self.assertEqual(calls, [])
        self.assertFalse(window._closing)

    def test_noninteractive_close_skips_modal_but_still_stops_children(self) -> None:
        calls: list[str] = []
        window = _window(calls, (self.active,))

        with patch("cutting_board.ui.main_window.ask_confirmation") as ask:
            window.close(notify=False)

        ask.assert_not_called()
        self.assertEqual(calls, ["settings", "launch", "scanner", "executor", "root"])

    def test_signal_handler_uses_noninteractive_close(self) -> None:
        window = Mock()
        handlers: dict[int, object] = {}

        with patch(
            "cutting_board.app.signal.signal",
            side_effect=lambda number, handler: handlers.setdefault(number, handler),
        ):
            _close_on_signal(window)

        handler = handlers[signal.SIGTERM]
        assert callable(handler)
        handler(signal.SIGTERM, None)
        window.close.assert_called_once_with(notify=False)


class ThemeLifecycleTests(unittest.TestCase):
    def test_listener_count_label_uses_singular_and_plural_nouns(self) -> None:
        self.assertEqual(_listener_count_label(1), "1 listener")
        self.assertEqual(_listener_count_label(2), "2 listeners")

    def test_settings_offer_all_persisted_theme_modes_in_english(self) -> None:
        self.assertEqual(
            THEME_MODE_CHOICES,
            (("System", "system"), ("Dark", "dark"), ("Light", "light")),
        )

    def test_settings_dialog_destroys_before_requesting_rebuild(self) -> None:
        calls: list[str] = []
        dialog = SettingsDialog.__new__(SettingsDialog)
        dialog._theme_mode = "dark"
        dialog.destroy = lambda: calls.append("dialog")
        dialog._on_theme_change = lambda mode: calls.append(f"theme:{mode}")

        dialog._choose_theme("light")

        self.assertEqual(calls, ["dialog", "theme:light"])

    def test_theme_rebuild_preserves_runtime_objects_and_does_not_schedule_poll(self) -> None:
        destroyed: list[str] = []

        class Root:
            def __init__(self) -> None:
                self.after_calls = 0
                self.cancelled: list[str] = []
                self.children = [Mock(destroy=lambda: destroyed.append("old-ui"))]

            def winfo_children(self) -> list[Mock]:
                return self.children

            def after_cancel(self, job: str) -> None:
                self.cancelled.append(job)

            def after(self, *args: object) -> None:
                del args
                self.after_calls += 1

        root = Root()
        settings = UISettings(theme_mode="dark")
        store = Mock()
        launch_controller = Mock()
        scan_controller = Mock()
        executor = Mock()
        window = CuttingBoardWindow.__new__(CuttingBoardWindow)
        window.root = root
        window.settings = settings
        window.settings_store = store
        window.launch_controller = launch_controller
        window.controller = scan_controller
        window.executor = executor
        window.tab = TAB_LAUNCH
        window._toast = Mock()
        window._toast_job = "toast-job"
        window._columns = 3
        window._body_columns = 3
        window._body_signature = ("old",)
        window._tiles = [Mock()]

        new_scroll = Mock()
        new_launch_panel = Mock()

        def build_layout() -> None:
            window.scroll = new_scroll
            window.launch_panel = new_launch_panel

        with (
            patch("cutting_board.ui.main_window.theme.apply_palette") as apply_palette,
            patch(
                "cutting_board.ui.main_window.theme.configure_theme",
                return_value={"body": ("Sans", 11, "normal")},
            ),
            patch.object(window, "_build_layout", side_effect=build_layout),
            patch.object(window, "render") as render,
        ):
            window._apply_theme_mode("light")

        self.assertEqual(settings.theme_mode, "light")
        store.save.assert_called_once_with(settings)
        apply_palette.assert_called_once_with("light")
        self.assertEqual(destroyed, ["old-ui"])
        self.assertEqual(root.cancelled, ["toast-job"])
        self.assertEqual(root.after_calls, 0)
        self.assertEqual(window.tab, TAB_LAUNCH)
        new_scroll.pack_forget.assert_called_once_with()
        new_launch_panel.pack.assert_called_once_with(fill="both", expand=True, side="top")
        render.assert_called_once_with()
        self.assertEqual(launch_controller.method_calls, [])
        self.assertEqual(scan_controller.method_calls, [])
        self.assertEqual(executor.method_calls, [])


class ExternalLaunchIntegrationTests(unittest.TestCase):
    def test_tiles_keep_section_left_axis_at_all_column_counts(self) -> None:
        self.assertEqual(CuttingBoardWindow._tile_grid_sticky(1), "w")
        self.assertEqual(CuttingBoardWindow._tile_grid_sticky(3), "w")

    def test_flat_header_controls_keep_pointer_and_keyboard_activation(self) -> None:
        widget = Mock()
        activated: list[str] = []

        _bind_action(widget, lambda: activated.append("yes"))

        sequences = [call.args[0] for call in widget.bind.call_args_list]
        self.assertEqual(sequences, ["<Button-1>", "<Return>", "<space>"])
        for index, call in enumerate(widget.bind.call_args_list):
            event = Mock(num=1 if index == 0 else None)
            self.assertEqual(call.args[1](event), "break")
        self.assertEqual(activated, ["yes", "yes", "yes"])
        widget.focus_set.assert_called_once_with()

    def test_toolbar_surfaces_use_hover_and_visible_focus_colours(self) -> None:
        self.assertEqual(
            _toolbar_surface_colours(False, False),
            (theme.CANVAS, theme.CANVAS),
        )
        self.assertEqual(
            _toolbar_surface_colours(True, False),
            (theme.SURFACE_ALT, theme.BORDER),
        )
        self.assertEqual(
            _toolbar_surface_colours(False, True),
            (theme.SURFACE_ALT, theme.ACCENT),
        )
        self.assertEqual(
            _segmented_surface_colours(False, False, True),
            (theme.SURFACE, theme.SURFACE),
        )
        self.assertEqual(
            _segmented_surface_colours(True, False, False),
            (theme.SURFACE_HOVER, theme.SURFACE_HOVER),
        )
        self.assertEqual(
            _segmented_surface_colours(False, True, False),
            (theme.SURFACE_ALT, theme.ACCENT),
        )

    def test_settings_gear_has_36px_geometry_and_accessible_logical_name(self) -> None:
        points = _gear_polygon_points(SETTINGS_HIT_TARGET)

        self.assertEqual(HEADER_HEIGHT, 56)
        self.assertEqual(SETTINGS_HIT_TARGET, 36)
        self.assertEqual(_SettingsGear.accessible_name, "Settings")
        self.assertEqual(len(points), 64)
        self.assertTrue(all(1 <= coordinate <= 35 for coordinate in points))

    def test_matching_external_listener_is_visible_but_never_stoppable(self) -> None:
        root = Path(tempfile.gettempdir()) / "dutypark"
        profile = LaunchProfile(
            id="dutypark",
            name="dutypark",
            project_root=str(root),
            tasks=(LaunchTask("Backend", ".", "./gradlew bootRun", 8080),),
        )
        stopped = ManagedTaskSnapshot(
            profile_id=profile.id,
            task_name="Backend",
            state=LaunchState.STOPPED,
            expected_port=8080,
        )
        calls: list[str] = []
        window = CuttingBoardWindow.__new__(CuttingBoardWindow)
        window.snapshot = make_snapshot(
            make_service(port=8080, project=make_project("dutypark", str(root)))
        )
        window.launch_controller = _LaunchController(calls, (stopped,))
        window._show_toast = Mock()

        view = window._launch_task_view(profile, profile.tasks[0])
        window._stop_launch_task(profile.id, "Backend")

        self.assertTrue(view.external)
        self.assertFalse(view.can_start)
        self.assertFalse(view.can_stop)
        self.assertEqual(window.launch_controller.stop_calls, 0)

    def test_profile_start_skips_external_task_and_starts_stopped_sibling(self) -> None:
        root = Path(tempfile.gettempdir()) / "dutypark"
        profile = LaunchProfile(
            id="dutypark",
            name="dutypark",
            project_root=str(root),
            tasks=(
                LaunchTask("Backend", ".", "./gradlew bootRun", 8080),
                LaunchTask("Frontend", "frontend", "npm run dev", 5173),
            ),
        )
        stopped = {
            task.name: ManagedTaskSnapshot(
                profile_id=profile.id,
                task_name=task.name,
                state=LaunchState.STOPPED,
                expected_port=task.expected_port,
            )
            for task in profile.tasks
        }
        controller = Mock()
        controller.profiles = (profile,)
        controller.snapshot.side_effect = lambda _profile_id, name: stopped[name]
        started: list[str] = []
        controller.start_task.side_effect = (
            lambda _profile_id, name: started.append(name) or stopped[name]
        )
        window = CuttingBoardWindow.__new__(CuttingBoardWindow)
        window.snapshot = make_snapshot(
            make_service(port=8080, project=make_project("dutypark", str(root)))
        )
        window.launch_controller = controller
        window.executor = _ImmediateExecutor()
        window.launch_action_results = queue.Queue()

        views = window._launch_profile_views()
        window._start_launch_profile(profile.id)

        self.assertTrue(views[0].can_start)
        self.assertEqual(started, ["Frontend"])


if __name__ == "__main__":
    unittest.main()
