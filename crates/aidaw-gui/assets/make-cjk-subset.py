"""Rebuild NotoSansSC-Regular-subset.otf after adding Chinese translations.

    pip install fonttools
    python make-cjk-subset.py NotoSansSC-Regular.otf

NotoSansSC-Regular.otf comes from https://github.com/notofonts/noto-cjk
(Sans/SubsetOTF/SC). The subset keeps all of GB2312, CJK punctuation,
full-width forms and every character used in src/i18n.rs.
"""

import pathlib
import subprocess
import sys

here = pathlib.Path(__file__).parent
chars = set()
for hi in range(0xA1, 0xF8):
    for lo in range(0xA1, 0xFF):
        try:
            chars.add(bytes([hi, lo]).decode("gb2312"))
        except UnicodeDecodeError:
            pass
src = (here.parent / "src" / "i18n.rs").read_text(encoding="utf-8")
chars |= {c for c in src if ord(c) > 0x2000}
chars |= {chr(c) for c in range(0x3000, 0x3040)}
chars |= {chr(c) for c in range(0xFF01, 0xFF5F)}
text = here / "cjk-chars.tmp"
text.write_text("".join(sorted(chars)), encoding="utf-8")
subprocess.run(
    [
        sys.executable, "-m", "fontTools.subset", sys.argv[1],
        f"--text-file={text}",
        f"--output-file={here / 'NotoSansSC-Regular-subset.otf'}",
        "--no-hinting", "--layout-features=", "--name-IDs=*",
    ],
    check=True,
)
text.unlink()
