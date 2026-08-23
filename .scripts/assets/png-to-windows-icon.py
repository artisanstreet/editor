from pathlib import Path
import sys

from PIL import Image


def main() -> None:
    source = Path(sys.argv[1])
    destination = Path(sys.argv[2])
    with Image.open(source) as image:
        image.convert("RGBA").save(
            destination,
            format="ICO",
            sizes=[
                (16, 16),
                (20, 20),
                (24, 24),
                (32, 32),
                (40, 40),
                (48, 48),
                (64, 64),
                (128, 128),
                (256, 256),
            ],
        )


if __name__ == "__main__":
    main()
