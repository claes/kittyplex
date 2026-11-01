"""Local shortcut help; the overlay has no kittyplex session identity or watchers."""
import textwrap

from kittens.tui.handler import Handler, result_handler
from kittens.tui.loop import Loop


class Help(Handler):
    def __init__(self, prefix="ctrl+shift+m"):
        self.prefix = prefix
        self.offset = 0

    def lines(self):
        text = [
            "Kittyplex shortcuts", "",
            "Press " + self.prefix + ", release, then:", "",
            "t / c   New session in a tab",
            "v       Vertical split (new pane on the right)",
            "h       Horizontal split (new pane below)",
            "w       Close this pane and its session",
            "n / p   Next / previous session in the group",
            "1-9     Select a session in creation order",
            "q       Close the entire group",
            "?       Show this help",
            "Escape  Cancel management mode", "",
            "Repeat the prefix to send it to the application.",
            "Use Kitty's controls to move and resize panes.",
        ]
        width = max(1, self.screen_size.cols)
        return [line for paragraph in text
                for line in (textwrap.wrap(paragraph, width=width) or [""])]

    def draw(self):
        lines = self.lines()
        height = max(0, self.screen_size.rows - 1)
        self.offset = min(max(0, self.offset), max(0, len(lines) - max(1, height)))
        self.write("\x1b[?25l\x1b[2J\x1b[H")
        for row, line in enumerate(lines[self.offset:self.offset + height], 1):
            self.write(f"\x1b[{row};1H" + line)
        footer = "Escape to close | Up/Down/PgUp/PgDn to scroll"
        self.write(f"\x1b[{max(1, self.screen_size.rows)};1H" + footer[:self.screen_size.cols])

    def initialize(self):
        self.draw()

    def on_resize(self, screen_size):
        super().on_resize(screen_size)
        self.draw()

    def on_key(self, event):
        if event.matches("escape"):
            self.quit_loop(0)
            return
        page = max(1, self.screen_size.rows - 2)
        for key, step in (("up", -1), ("down", 1), ("page_up", -page), ("page_down", page)):
            if event.matches(key):
                self.offset += step
                self.draw()
                return

    def on_interrupt(self):
        self.quit_loop(0)

    def on_eot(self):
        self.quit_loop(0)


def main(args):
    Loop().loop(Help(args[1] if len(args) > 1 else "ctrl+shift+m"))


@result_handler()
def handle_result(args, answer, target_window_id, boss):
    pass
