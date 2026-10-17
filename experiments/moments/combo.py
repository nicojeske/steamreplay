"""Rank-average of two or more methods' curves (a no-training fusion).  uv run python combo.py name m1 m2 …"""

import sys

import numpy as np

from common import OUT, save_method, sessions
from eval import curves_1hz

name, methods = sys.argv[1], sys.argv[2:]
for s in sessions():
    curve = curves_1hz(s, methods).mean(axis=1)
    save_method(s, name, curve, 1.0, 0.0)
