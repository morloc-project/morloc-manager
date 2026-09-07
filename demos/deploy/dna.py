_COMPLEMENT = str.maketrans("ACGTacgt", "TGCAtgca")


def revcomp(seq):
    """Reverse complement a DNA sequence, preserving case."""
    return seq.translate(_COMPLEMENT)[::-1]
