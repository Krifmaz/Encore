"""make.py before|after OUT: writes the screenshot test for the old (before) or kit (after) names."""
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
side, out = sys.argv[1], sys.argv[2]
t = open(os.path.join(here, "ScreensTest.kt.in")).read()
t = t.replace("@IMPORTS@", open(os.path.join(here, f"imports-{side}.txt")).read().rstrip("\n"))
t = t.replace("@BUTTON@", "EncoreButton" if side == "before" else "KitButton").replace("@FIELD@", "EncoreField" if side == "before" else "KitField")
os.makedirs(os.path.dirname(out), exist_ok=True)
open(out, "w").write(t)
