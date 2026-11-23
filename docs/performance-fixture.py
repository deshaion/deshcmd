"""Create the phase 1 benchmark fixture in a new directory."""
import pathlib
import random
import sys

root = pathlib.Path(sys.argv[1]).resolve()
root.mkdir()
(root / "empty").mkdir()
for count in (100, 10_000, 100_000):
    directory = root / str(count)
    directory.mkdir()
    order = list(range(count))
    random.Random(42).shuffle(order)
    for number in order:
        (directory / f"file{number}.txt").touch()
    (directory / "nested").mkdir()
    (directory / "nested" / "child.txt").touch()
    (directory / ".hidden").touch()
    (directory / "descript.ion").write_text("file0.txt A comment\n", encoding="utf-8")
    try:
        (directory / "file-link").symlink_to("file0.txt")
        (directory / "broken-link").symlink_to("missing")
    except (OSError, NotImplementedError):
        pass
plain = root / "plain"
plain.mkdir()
for number in range(100_000):
    (plain / f"file{number}.txt").touch()
