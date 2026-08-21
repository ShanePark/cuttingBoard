from __future__ import annotations

import unittest
from types import SimpleNamespace
from unittest.mock import patch

from cutting_board.ui import theme
from cutting_board.ui.widgets import (
    ServiceTile,
    _action_style,
    _browser_link_label,
    _card_style,
    _ellipsis_to_width,
    _live_uptime_seconds,
    _next_action_key,
    _origin_colour,
    _port_badge_labels,
    _power_action_geometry,
    _section_accent,
)
from tests.helpers import make_service


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


class ThemeTests(unittest.TestCase):
    def tearDown(self) -> None:
        theme.apply_palette("dark")

    def test_every_text_style_is_at_least_ten_points(self) -> None:
        fonts = theme._font_specs("Sans", "Mono")

        self.assertTrue(fonts)
        self.assertGreaterEqual(min(size for _family, size, _weight in fonts.values()), 10)

    def test_both_palettes_clear_normal_text_contrast(self) -> None:
        for mode in ("dark", "light"):
            theme.apply_palette(mode)
            foregrounds = {
                theme.TEXT,
                theme.TEXT_MUTED,
                theme.TEXT_DIM,
                theme.ACCENT,
                theme.VIOLET,
                theme.DANGER,
                theme.OK,
                theme.WARNING,
                *theme.CATEGORY_COLORS.values(),
            }
            backgrounds = (
                theme.CANVAS,
                theme.SURFACE,
                theme.SURFACE_ALT,
                theme.SURFACE_HOVER,
            )

            for background in backgrounds:
                for foreground in foregrounds:
                    with self.subTest(
                        mode=mode,
                        foreground=foreground,
                        background=background,
                    ):
                        self.assertGreaterEqual(
                            _contrast_ratio(foreground, background),
                            4.5,
                        )

            for hover_background in (theme.ACCENT_DIM, theme.DANGER_DIM, theme.BORDER):
                with self.subTest(mode=mode, hover_background=hover_background):
                    self.assertGreaterEqual(
                        _contrast_ratio(theme.TEXT, hover_background),
                        4.5,
                    )

            for accent_background in (theme.ACCENT, theme.ACCENT_HOVER):
                with self.subTest(mode=mode, accent_background=accent_background):
                    self.assertGreaterEqual(
                        _contrast_ratio(theme.ON_ACCENT, accent_background),
                        4.5,
                    )

    def test_apply_palette_replaces_the_complete_semantic_set(self) -> None:
        theme.apply_palette("light")
        light_categories = theme.CATEGORY_COLORS
        self.assertEqual(theme.CURRENT_THEME, "light")
        self.assertEqual(theme.CANVAS, theme.LIGHT_PALETTE.canvas)

        resolved = theme.apply_palette("invalid")

        self.assertEqual(resolved, "dark")
        self.assertEqual(theme.CURRENT_THEME, "dark")
        self.assertEqual(theme.CANVAS, theme.DARK_PALETTE.canvas)
        self.assertEqual(theme.CATEGORY_COLORS, dict(theme.DARK_PALETTE.category_colors))
        self.assertIsNot(theme.CATEGORY_COLORS, light_categories)

    def test_widget_defaults_resolve_after_each_palette_application(self) -> None:
        theme.apply_palette("dark")
        dark = (_origin_colour("agent"), _origin_colour("ide"), _section_accent(None))

        theme.apply_palette("light")
        light = (_origin_colour("agent"), _origin_colour("ide"), _section_accent(None))

        self.assertEqual(dark, (theme.DARK_PALETTE.violet, theme.DARK_PALETTE.accent, theme.DARK_PALETTE.text_dim))
        self.assertEqual(
            light,
            (
                theme.LIGHT_PALETTE.violet,
                theme.LIGHT_PALETTE.accent,
                theme.LIGHT_PALETTE.text_dim,
            ),
        )
        self.assertNotEqual(dark, light)

    def test_system_theme_detection_is_macos_only_and_falls_back_dark(self) -> None:
        self.assertEqual(
            theme.resolve_theme_mode(
                "system",
                platform_name="darwin",
                system_style_reader=lambda: "light",
            ),
            "light",
        )
        self.assertEqual(
            theme.resolve_theme_mode(
                "system",
                platform_name="darwin",
                system_style_reader=lambda: None,
            ),
            "dark",
        )
        self.assertEqual(
            theme.resolve_theme_mode(
                "system",
                platform_name="linux",
                system_style_reader=lambda: "light",
            ),
            "dark",
        )

    def test_macos_reader_distinguishes_light_dark_and_failure(self) -> None:
        cases = (
            (0, '{\n    AppleInterfaceStyle = Dark;\n}', "dark"),
            (0, "{\n    AppleAccentColor = 4;\n}", "light"),
            (1, "", None),
        )
        for returncode, stdout, expected in cases:
            with self.subTest(returncode=returncode, expected=expected):
                completed = SimpleNamespace(returncode=returncode, stdout=stdout)
                with patch.object(theme.subprocess, "run", return_value=completed):
                    self.assertEqual(theme._read_macos_interface_style(), expected)

    def test_on_accent_is_distinct_from_regular_text_when_needed(self) -> None:
        theme.apply_palette("light")
        self.assertNotEqual(theme.ON_ACCENT, theme.TEXT)
        self.assertGreaterEqual(
            _contrast_ratio(theme.ON_ACCENT, theme.ACCENT),
            4.5,
        )

    def test_card_metrics_fit_common_window_widths(self) -> None:
        card_stride = theme.TILE_SPAN + theme.GRID_GUTTER

        def columns(width: int) -> int:
            usable = max(0, width - (theme.TILE_PAD + 4) * 2)
            return max(1, usable // card_stride)

        self.assertEqual(columns(560), 1)
        self.assertEqual(columns(900), 3)
        self.assertEqual(columns(1280), 4)
        visible_card_height = (theme.TILE_HEIGHT + theme.TILE_PAD + 2) - 6
        self.assertGreaterEqual(visible_card_height, 124)
        self.assertLessEqual(visible_card_height, 136)
        self.assertEqual(
            theme.TILE_HEIGHT + theme.TILE_PAD * 2,
            136,
        )

    def test_surface_spacing_and_corner_tokens_form_a_consistent_scale(self) -> None:
        self.assertEqual(
            (theme.SPACE_XS, theme.SPACE_SM, theme.SPACE_MD, theme.SPACE_LG),
            (4, 8, 12, 16),
        )
        self.assertEqual(theme.CARD_RADIUS, theme.SPACE_LG)
        self.assertEqual(theme.ICON_WELL_SIZE, 56)
        self.assertLess(theme.ICON_WELL_RADIUS, theme.CARD_RADIUS)
        self.assertGreaterEqual(theme.CONTROL_HIT_SIZE, 36)
        self.assertEqual(theme.CONTROL_ICON_SIZE, 18)


class WidgetLogicTests(unittest.TestCase):
    def tearDown(self) -> None:
        theme.apply_palette("dark")

    def test_live_uptime_advances_between_scans_without_moving_backwards(self) -> None:
        self.assertEqual(
            _live_uptime_seconds(60, 1_000.0, now=1_061.9),
            61,
        )
        self.assertEqual(
            _live_uptime_seconds(62, 1_000.0, now=1_061.9),
            62,
        )
        self.assertIsNone(_live_uptime_seconds(None, 1_000.0, now=1_061.9))

    def test_port_badges_stay_quiet_and_report_hidden_count(self) -> None:
        self.assertEqual(_port_badge_labels(()), ())
        self.assertEqual(_port_badge_labels((5173,)), ("5173",))
        self.assertEqual(_port_badge_labels((5173, 8080)), ("5173", "8080"))
        self.assertEqual(
            _port_badge_labels((5173, 8080, 9090)),
            ("5173", "+2"),
        )

    def test_browser_link_label_normalizes_local_hosts_but_keeps_destination(self) -> None:
        cases = (
            ("http://127.0.0.1:5173", "localhost:5173"),
            ("http://0.0.0.0:8080", "localhost:8080"),
            ("http://*:5173", "localhost:5173"),
            ("http://[::]:5173", "localhost:5173"),
            ("http://[::1]:3000/app#dev", "localhost:3000/app#dev"),
            (
                "https://localhost.localdomain:8443/admin?tab=logs",
                "localhost:8443/admin?tab=logs",
            ),
            ("https://192.168.1.4:9443/health", "192.168.1.4:9443/health"),
        )

        for url, expected in cases:
            with self.subTest(url=url):
                self.assertEqual(_browser_link_label(url), expected)

        malformed = "http://[::1"
        self.assertEqual(_browser_link_label(malformed), malformed)

    def test_pixel_ellipsis_handles_unicode_and_mixed_width_names(self) -> None:
        def measure(text: str) -> int:
            return sum(11 if character > "\u007f" else 6 for character in text)

        for name in (
            "DéveloppementÜberServiceBackend",
            "dutypark-développement-SpringBoot-service",
        ):
            with self.subTest(name=name):
                fitted = _ellipsis_to_width(name, 96, measure)
                self.assertTrue(fitted.endswith("…"))
                self.assertLessEqual(measure(fitted), 96)
                self.assertGreater(measure(name), 96)

        self.assertEqual(_ellipsis_to_width("Vite", 96, measure), "Vite")

    def test_action_cycle_wraps_and_defaults_to_details(self) -> None:
        keys = ("details", "open", "terminate")

        self.assertEqual(_next_action_key(keys, None, 1), "details")
        self.assertEqual(_next_action_key(keys, "details", 1), "open")
        self.assertEqual(_next_action_key(keys, "details", -1), "terminate")
        self.assertEqual(_next_action_key(keys, "terminate", 1), "details")

    def test_busy_service_keeps_details_and_open_but_disables_terminate(self) -> None:
        tile = ServiceTile.__new__(ServiceTile)
        tile.service = make_service()
        tile._busy = True
        calls: list[str] = []
        tile._on_details = lambda _service: calls.append("details")
        tile._on_open = lambda _service: calls.append("open")
        tile._on_terminate = lambda _service: calls.append("terminate")

        tile._invoke_action("details")
        tile._invoke_action("open")
        tile._invoke_action("terminate")

        self.assertEqual(calls, ["details", "open"])

    def test_power_control_has_a_large_hit_target_around_a_quiet_visual(self) -> None:
        hit, visual, centre = _power_action_geometry(276)

        self.assertEqual(hit[2] - hit[0], 36)
        self.assertEqual(hit[3] - hit[1], 36)
        self.assertEqual(visual[2] - visual[0], theme.CONTROL_SIZE)
        self.assertEqual(visual[3] - visual[1], theme.CONTROL_SIZE)
        self.assertEqual((sum(hit[::2]) / 2, sum(hit[1::2]) / 2), centre)
        self.assertGreater(hit[0], 0)
        self.assertLess(hit[2], 276)
        title_right = 276 - theme.SPACE_MD - theme.CONTROL_SIZE - theme.SPACE_SM
        self.assertLess(title_right, hit[0])
        self.assertLess(hit[3], 82 - 9)

    def test_power_and_open_action_states_stay_distinct_and_accessible(self) -> None:
        for mode in ("dark", "light"):
            theme.apply_palette(mode)
            states = (
                _action_style(
                    "terminate",
                    enabled=True,
                    hovered=False,
                    selected=False,
                    card_hovered=False,
                ),
                _action_style(
                    "terminate",
                    enabled=True,
                    hovered=True,
                    selected=False,
                    card_hovered=True,
                ),
                _action_style(
                    "terminate",
                    enabled=False,
                    hovered=False,
                    selected=False,
                    card_hovered=False,
                ),
                _action_style(
                    "open",
                    enabled=True,
                    hovered=False,
                    selected=False,
                    card_hovered=False,
                ),
                _action_style(
                    "open",
                    enabled=True,
                    hovered=True,
                    selected=False,
                    card_hovered=True,
                ),
                _action_style(
                    "open",
                    enabled=True,
                    hovered=False,
                    selected=True,
                    card_hovered=False,
                ),
            )
            with self.subTest(mode=mode):
                self.assertEqual(states[0].foreground, theme.DANGER)
                self.assertEqual(states[1].fill, theme.SURFACE_HOVER)
                self.assertEqual(states[1].outline, theme.BORDER)
                self.assertEqual(states[2].foreground, theme.TEXT_DIM)
                self.assertEqual(states[3].foreground, theme.ACCENT)
                self.assertEqual(states[4].foreground, theme.ACCENT_HOVER)
                self.assertEqual(states[5].outline, "")
                self.assertEqual(states[5].foreground, theme.ACCENT_HOVER)
            for state in states:
                with self.subTest(mode=mode, state=state):
                    self.assertGreaterEqual(
                        _contrast_ratio(state.foreground, state.fill),
                        4.5,
                    )

    def test_card_focus_does_not_leave_a_selected_blue_outline(self) -> None:
        for mode in ("dark", "light"):
            theme.apply_palette(mode)
            idle = _card_style(hovered=False, focused=False)
            focused = _card_style(hovered=False, focused=True)
            hovered = _card_style(hovered=True, focused=True)

            with self.subTest(mode=mode):
                self.assertEqual(focused, idle)
                self.assertEqual(focused.outline, theme.HAIRLINE)
                self.assertEqual(hovered.outline, theme.BORDER)
                self.assertNotEqual(focused.outline, theme.ACCENT)
                self.assertNotEqual(hovered.outline, theme.ACCENT)
                self.assertEqual((focused.width, hovered.width), (1, 1))


if __name__ == "__main__":
    unittest.main()
