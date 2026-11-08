"""Select one fenced shell example from an exact README section."""
import re


def command_block(readme, heading):
    match = re.search(rf"(?m)^## {re.escape(heading)}\n", readme)
    if not match:
        raise ValueError(f"missing README section: {heading}")
    section = re.split(r"(?m)^## ", readme[match.end():], maxsplit=1)[0]
    blocks = re.findall(r"```sh\n(.*?)\n```", section, re.DOTALL)
    if len(blocks) != 1 or not blocks[0].strip():
        raise ValueError(f"expected one nonempty shell example: {heading}")
    return blocks[0].strip()
