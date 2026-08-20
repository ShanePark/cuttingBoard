from __future__ import annotations

import webbrowser


def open_url(url: str) -> bool:
    if not url.startswith(("http://", "https://")):
        return False
    return bool(webbrowser.open(url, new=2, autoraise=True))
