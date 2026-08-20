from __future__ import annotations

import inspect
import re
import unittest
from collections.abc import Callable
from types import SimpleNamespace
from unittest.mock import Mock

from cutting_board.ui.dialogs import (
    SCRIM_ALPHA,
    ConfirmDialog,
    ContainerDetailDialog,
    ModalBackdrop,
    ServiceDetailDialog,
    SettingsDialog,
    _centred_modal_geometry,
    configure_detail_dismiss,
)

EventHandler = Callable[[SimpleNamespace], object]


class _BindingTarget:
    def __init__(self) -> None:
        self.events: list[str] = []
        self.bindings: dict[str, EventHandler] = {}
        self.binding_ids: dict[str, str] = {}
        self.unbind_calls: list[tuple[str, str | None]] = []
        self.width = 900
        self.height = 640
        self.root_x = 120
        self.root_y = 80
        self.screen_width = 1440
        self.screen_height = 900

    def bind(
        self,
        sequence: str,
        handler: EventHandler,
        add: str | None = None,
    ) -> str:
        del add
        binding_id = f"binding-{len(self.binding_ids) + 1}"
        self.bindings[sequence] = handler
        self.binding_ids[sequence] = binding_id
        return binding_id

    def unbind(self, sequence: str, binding_id: str | None = None) -> None:
        self.unbind_calls.append((sequence, binding_id))

    def winfo_toplevel(self) -> _BindingTarget:
        return self

    def winfo_width(self) -> int:
        return self.width

    def winfo_height(self) -> int:
        return self.height

    def winfo_rootx(self) -> int:
        return self.root_x

    def winfo_rooty(self) -> int:
        return self.root_y

    def winfo_screenwidth(self) -> int:
        return self.screen_width

    def winfo_screenheight(self) -> int:
        return self.screen_height


class _Window(_BindingTarget):
    def __init__(self, master: _BindingTarget, *, bg: str) -> None:
        super().__init__()
        self.events = master.events
        self.master = master
        self.bg = bg
        self.exists = True
        self.withdraw_calls = 0
        self.deiconify_calls = 0
        self.override_redirect: bool | None = None
        self.transient_target: object | None = None
        self.attribute_calls: list[tuple[str, float]] = []
        self.configure_calls: list[dict[str, object]] = []
        self.geometries: list[str] = []
        self.lift_targets: list[object] = []
        self.current_grab: object | None = None
        self.grab_set_calls = 0
        self.grab_release_calls = 0
        self.destroy_calls = 0
        self.update_idletasks_calls = 0
        self.wait_visibility_calls = 0

    def withdraw(self) -> None:
        self.withdraw_calls += 1

    def overrideredirect(self, value: bool) -> None:
        self.override_redirect = value

    def transient(self, target: object) -> None:
        self.transient_target = target

    def attributes(self, name: str, value: float) -> None:
        self.attribute_calls.append((name, value))

    def configure(self, **kwargs: object) -> None:
        self.configure_calls.append(kwargs)

    def geometry(self, value: str) -> None:
        self.geometries.append(value)

    def deiconify(self) -> None:
        self.deiconify_calls += 1
        self.events.append("scrim.deiconify")

    def update_idletasks(self) -> None:
        self.update_idletasks_calls += 1
        self.events.append("scrim.update_idletasks")

    def wait_visibility(self) -> None:
        self.wait_visibility_calls += 1
        self.events.append("scrim.wait_visibility")

    def lift(self, target: object) -> None:
        self.lift_targets.append(target)
        self.events.append("scrim.lift")

    def grab_set(self) -> None:
        self.grab_set_calls += 1
        self.current_grab = self
        self.events.append("scrim.grab_set")

    def grab_current(self) -> object | None:
        return self.current_grab

    def grab_release(self) -> None:
        self.grab_release_calls += 1
        self.current_grab = None

    def winfo_exists(self) -> bool:
        return self.exists

    def destroy(self) -> None:
        self.destroy_calls += 1
        self.exists = False
        handler = self.bindings.get("<Destroy>")
        if handler is not None:
            handler(_event(self))


class _Dialog(_BindingTarget):
    def __init__(self) -> None:
        super().__init__()
        self.width = 500
        self.height = 300
        self.exists = True
        self.destroy_calls = 0
        self.focus_calls = 0
        self.update_idletasks_calls = 0
        self.deiconify_calls = 0
        self.geometries: list[str] = []
        self.lift_targets: list[object] = []
        self.idle_callbacks: list[Callable[[], None]] = []
        self.frame_x = 0
        self.frame_y = 0
        self.titlebar_height = 32

    def focus_set(self) -> None:
        self.focus_calls += 1
        self.events.append("dialog.focus_set")

    def lift(self, target: object) -> None:
        self.lift_targets.append(target)
        self.events.append("dialog.lift")

    def deiconify(self) -> None:
        self.deiconify_calls += 1
        self.events.append("dialog.deiconify")

    def update_idletasks(self) -> None:
        self.update_idletasks_calls += 1
        self.events.append("dialog.update_idletasks")

    def geometry(self, value: str) -> None:
        self.geometries.append(value)
        match = re.fullmatch(r"([+-]\d+)([+-]\d+)", value)
        assert match is not None
        self.frame_x = int(match.group(1))
        self.frame_y = int(match.group(2))
        self.root_x = self.frame_x
        self.root_y = self.frame_y + self.titlebar_height

    def winfo_x(self) -> int:
        return self.frame_x

    def winfo_y(self) -> int:
        return self.frame_y

    def after_idle(self, callback: Callable[[], None]) -> None:
        self.idle_callbacks.append(callback)

    def run_idle_callbacks(self) -> None:
        callbacks, self.idle_callbacks = self.idle_callbacks, []
        for callback in callbacks:
            callback()

    def destroy(self) -> None:
        self.destroy_calls += 1
        self.exists = False
        handler = self.bindings.get("<Destroy>")
        if handler is not None:
            handler(_event(self))


def _event(widget: object) -> SimpleNamespace:
    return SimpleNamespace(widget=widget)


def _backdrop() -> tuple[ModalBackdrop, _BindingTarget, _Window]:
    parent = _BindingTarget()
    windows: list[_Window] = []

    def factory(master: _BindingTarget, *, bg: str) -> _Window:
        window = _Window(master, bg=bg)
        windows.append(window)
        return window

    backdrop = ModalBackdrop(
        parent,  # type: ignore[arg-type]
        _window_factory=factory,  # type: ignore[arg-type]
    )
    return backdrop, parent, windows[0]


class ModalBackdropTests(unittest.TestCase):
    def test_exact_parent_centre_preserves_global_monitor_coordinates(self) -> None:
        self.assertEqual(
            _centred_modal_geometry(
                parent_x=34,
                parent_y=64,
                parent_width=1280,
                parent_height=820,
                dialog_width=500,
                dialog_height=300,
            ),
            "+424+324",
        )
        self.assertEqual(
            _centred_modal_geometry(
                parent_x=2560,
                parent_y=30,
                parent_width=1152,
                parent_height=1009,
                dialog_width=662,
                dialog_height=702,
            ),
            "+2805+183",
        )
        self.assertEqual(
            _centred_modal_geometry(
                parent_x=-1440,
                parent_y=-120,
                parent_width=1440,
                parent_height=900,
                dialog_width=600,
                dialog_height=500,
            ),
            "-1020+80",
        )

    def test_scrim_is_borderless_dimmed_and_initially_hidden(self) -> None:
        backdrop, parent, window = _backdrop()

        self.assertIs(backdrop.parent, parent)
        self.assertEqual(window.bg, "#000000")
        self.assertEqual(window.withdraw_calls, 1)
        self.assertTrue(window.override_redirect)
        self.assertIs(window.transient_target, parent)
        self.assertEqual(window.attribute_calls, [("-alpha", SCRIM_ALPHA)])
        self.assertIn({"cursor": "arrow"}, window.configure_calls)

    def test_activation_covers_parent_without_grabbing_dialog_events(self) -> None:
        backdrop, parent, window = _backdrop()
        dialog = _Dialog()
        dialog.events = parent.events

        backdrop.activate(dialog, on_outside=dialog.destroy)  # type: ignore[arg-type]

        self.assertEqual(window.geometries, ["900x640+120+80"])
        self.assertEqual(window.deiconify_calls, 1)
        self.assertEqual(window.update_idletasks_calls, 1)
        self.assertEqual(window.wait_visibility_calls, 1)
        self.assertEqual(dialog.deiconify_calls, 1)
        self.assertEqual(window.grab_set_calls, 0)
        self.assertIsNone(window.current_grab)
        self.assertEqual(dialog.focus_calls, 1)
        self.assertEqual(dialog.update_idletasks_calls, 3)
        self.assertEqual(dialog.geometries, ["+320+250", "+320+218"])
        self.assertEqual((dialog.winfo_rootx(), dialog.winfo_rooty()), (320, 250))
        self.assertIs(dialog.lift_targets[-1], window)
        self.assertIs(window.lift_targets[-1], parent)
        self.assertLess(parent.events.index("scrim.deiconify"), parent.events.index("dialog.deiconify"))

        dialog.run_idle_callbacks()

        self.assertEqual(dialog.deiconify_calls, 2)
        self.assertEqual(dialog.focus_calls, 2)
        self.assertEqual((dialog.winfo_rootx(), dialog.winfo_rooty()), (320, 250))

    def test_scrim_click_dismisses_once_and_never_clicks_through(self) -> None:
        backdrop, parent, window = _backdrop()
        dialog = _Dialog()
        dismissed = Mock(side_effect=dialog.destroy)
        backdrop.activate(dialog, on_outside=dismissed)  # type: ignore[arg-type]

        result = window.bindings["<Button-1>"](_event(window))
        second_result = window.bindings["<Button-1>"](_event(window))
        dialog.run_idle_callbacks()

        self.assertEqual(result, "break")
        self.assertEqual(second_result, "break")
        dismissed.assert_called_once_with()
        self.assertEqual(window.grab_release_calls, 0)
        self.assertEqual(window.destroy_calls, 1)
        self.assertEqual(dialog.deiconify_calls, 1)
        self.assertCountEqual(
            parent.unbind_calls,
            [
                ("<Configure>", parent.binding_ids["<Configure>"]),
                ("<Destroy>", parent.binding_ids["<Destroy>"]),
            ],
        )

    def test_non_dismissible_form_scrim_blocks_click_without_closing(self) -> None:
        backdrop, _parent, window = _backdrop()
        dialog = _Dialog()
        backdrop.activate(dialog, on_outside=None)  # type: ignore[arg-type]

        result = window.bindings["<Button-1>"](_event(window))

        self.assertEqual(result, "break")
        self.assertEqual(dialog.destroy_calls, 0)
        self.assertTrue(window.exists)

    def test_dialog_destroy_removes_parent_bindings_and_scrim(self) -> None:
        backdrop, parent, window = _backdrop()
        dialog = _Dialog()
        backdrop.activate(dialog, on_outside=dialog.destroy)  # type: ignore[arg-type]

        dialog.destroy()

        self.assertEqual(window.grab_release_calls, 0)
        self.assertEqual(window.destroy_calls, 1)
        self.assertEqual(len(parent.unbind_calls), 2)

    def test_parent_move_and_resize_resyncs_geometry_but_child_event_does_not(self) -> None:
        backdrop, parent, window = _backdrop()
        dialog = _Dialog()
        backdrop.activate(dialog, on_outside=None)  # type: ignore[arg-type]
        parent.width = 1024
        parent.height = 720
        parent.root_x = 50
        parent.root_y = 35
        configure_handler = parent.bindings["<Configure>"]

        configure_handler(_event(dialog))
        configure_handler(_event(parent))

        self.assertEqual(window.geometries, ["900x640+120+80", "1024x720+50+35"])
        self.assertEqual(
            dialog.geometries,
            ["+320+250", "+320+218", "+312+245", "+312+213"],
        )
        self.assertEqual((dialog.winfo_rootx(), dialog.winfo_rooty()), (312, 245))

    def test_parent_destroy_closes_scrim_and_cleanup_is_idempotent(self) -> None:
        backdrop, parent, window = _backdrop()
        dialog = _Dialog()
        backdrop.activate(dialog, on_outside=None)  # type: ignore[arg-type]

        parent.bindings["<Destroy>"](_event(parent))
        backdrop.close()

        self.assertEqual(window.grab_release_calls, 0)
        self.assertEqual(window.destroy_calls, 1)
        self.assertEqual(len(parent.unbind_calls), 2)

    def test_detail_policy_keeps_escape_and_uses_scrim_dismissal(self) -> None:
        dialog = _Dialog()
        backdrop = Mock()

        configure_detail_dismiss(
            dialog,  # type: ignore[arg-type]
            backdrop,
        )

        backdrop.activate.assert_called_once_with(dialog, on_outside=dialog.destroy)
        dialog.bindings["<Escape>"](_event(dialog))
        self.assertEqual(dialog.destroy_calls, 1)

    def test_dialog_initializers_assign_explicit_outside_policies(self) -> None:
        self.assertIn(
            "configure_detail_dismiss(self, backdrop)",
            inspect.getsource(ContainerDetailDialog.__init__),
        )
        self.assertIn(
            "configure_detail_dismiss(self, backdrop)",
            inspect.getsource(ServiceDetailDialog.__init__),
        )
        confirm_source = inspect.getsource(ConfirmDialog.__init__)
        settings_source = inspect.getsource(SettingsDialog.__init__)
        self.assertIn("on_outside=lambda: self._answer(False)", confirm_source)
        self.assertIn('self.bind("<Escape>", lambda _event: self._answer(False))', confirm_source)
        self.assertIn("on_outside=self.destroy", settings_source)
        self.assertIn('self.bind("<Escape>", lambda _event: self.destroy())', settings_source)
        self.assertNotIn("grab_set(", inspect.getsource(ModalBackdrop))
        self.assertNotIn("grab_set_global", inspect.getsource(ModalBackdrop))

if __name__ == "__main__":
    unittest.main()
