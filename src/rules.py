import argparse
from collections import namedtuple
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
Token = namedtuple("Token", "kind text line value")
Violation = namedtuple("Violation", "path line rule message")
LICENSE = re.compile(r"SPDX-License-Identifier:|Copyright \(c\)|Copyright ©|Licensed under", re.IGNORECASE)
SQL = (
    re.compile(r"\bSELECT\s+(?:DISTINCT\s+)?(?:\*|[\w.]+(?:\s+AS\s+\w+)?\s*(?:,|\s+FROM\b))"),
    re.compile(r"\bINSERT\s+(?:OR\s+(?:REPLACE|IGNORE|ABORT|FAIL|ROLLBACK)\s+)?INTO\s+\w+"),
    re.compile(r"\bREPLACE\s+INTO\s+\w+"),
    re.compile(r"\bUPDATE\s+\w+\s+SET\s+\w+\s*="),
    re.compile(r"\bDELETE\s+FROM\s+\w+"),
    re.compile(r"\b(?:CREATE|DROP)\s+(?:UNIQUE\s+|TEMP\s+|TEMPORARY\s+|VIRTUAL\s+)?(?:TABLE|INDEX|VIEW|TRIGGER)\b"),
    re.compile(r"\bALTER\s+TABLE\s+\w+"),
    re.compile(r"\bPRAGMA\s+\w+"),
    re.compile(r"\bWITH\s+(?:RECURSIVE\s+)?\w+(?:\s*\([\w\s,]*\))?\s+AS\s*\("),
    re.compile(r"\b(?:BEGIN\s+(?:IMMEDIATE|EXCLUSIVE|DEFERRED)|VACUUM\s+INTO|ATTACH\s+DATABASE)\b"),
)
TEST_ATTRIBUTES = {"test", "bench"}
TEST_CRATES = {"test-support"}


class LexError(ValueError):
    def __init__(self, line, message):
        super().__init__(message)
        self.line = line


def identifier_start(character):
    return character == "_" or character.isalpha()


def identifier_part(character):
    return character == "_" or character.isalnum()


def quoted_end(source, start, quote, line):
    index = start
    while index < len(source):
        character = source[index]
        if character == "\\":
            index += 2
        elif character == quote:
            return index + 1
        else:
            index += 1
    raise LexError(line, "unterminated literal")


def raw_end(source, start, line):
    hashes = 0
    while source.startswith("#", start + hashes):
        hashes += 1
    if not source.startswith('"', start + hashes):
        return None
    closing = '"' + "#" * hashes
    end = source.find(closing, start + hashes + 1)
    if end < 0:
        raise LexError(line, "unterminated raw string")
    return end + len(closing), start + hashes + 1, end


def block_end(source, start, line):
    depth = 0
    index = start
    while index < len(source):
        if source.startswith("/*", index):
            depth += 1
            index += 2
        elif source.startswith("*/", index):
            depth -= 1
            index += 2
            if depth == 0:
                return index
        else:
            index += 1
    raise LexError(line, "unterminated block comment")


def tokenize(source):
    tokens = []
    index = 0
    line = 1
    while index < len(source):
        character = source[index]
        start = index
        kind = "punct"
        value = None
        if character.isspace():
            index += 1
            line += character == "\n"
            continue
        if source.startswith("//", index):
            end = source.find("\n", index)
            index = len(source) if end < 0 else end
            kind = "comment"
        elif source.startswith("/*", index):
            index = block_end(source, index, line)
            kind = "comment"
        elif character == '"':
            index = quoted_end(source, index + 1, '"', line)
            kind, value = "string", source[start + 1:index - 1]
        elif character == "'":
            if source.startswith("\\", index + 1):
                index = quoted_end(source, index + 1, "'", line)
                kind = "char"
            elif source.startswith("'", index + 2):
                index += 3
                kind = "char"
            elif index + 1 < len(source) and identifier_start(source[index + 1]):
                index += 1
                while index < len(source) and identifier_part(source[index]):
                    index += 1
                kind = "lifetime"
            else:
                index += 1
        elif identifier_start(character) or character.isdigit():
            while index < len(source) and identifier_part(source[index]):
                index += 1
            word = source[start:index]
            kind = "number" if character.isdigit() else "ident"
            following = source[index:index + 1]
            if word in ("r", "br", "cr") and following in ('"', "#"):
                raw = raw_end(source, index, line)
                if raw is not None:
                    index, inner_start, inner_end = raw
                    kind, value = "string", source[inner_start:inner_end]
                elif word == "r" and identifier_start(source[index + 1:index + 2]):
                    index += 1
                    while index < len(source) and identifier_part(source[index]):
                        index += 1
            elif word in ("b", "c") and following == '"':
                index = quoted_end(source, index + 1, '"', line)
                kind, value = "string", source[start + 2:index - 1]
            elif word == "b" and following == "'":
                index = quoted_end(source, index + 1, "'", line)
                kind = "char"
        else:
            index += 1
        text = source[start:index]
        tokens.append(Token(kind, text, line, value))
        line += text.count("\n")
    return tokens


def matching(tokens, start):
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = []
    for index in range(start, len(tokens)):
        text = tokens[index].text if tokens[index].kind == "punct" else None
        if text in pairs:
            stack.append(pairs[text])
        elif text in (")", "]", "}"):
            if not stack or stack.pop() != text:
                raise LexError(tokens[index].line, f"unbalanced {text!r}")
            if not stack:
                return index
    raise LexError(tokens[start].line, "unclosed delimiter")


def code(tokens):
    return [token for token in tokens if token.kind != "comment"]


def license_header(tokens, position):
    leading = []
    for token in tokens:
        if token.kind != "comment":
            break
        leading.append(token)
    return position < len(leading) and any(LICENSE.search(token.text) for token in leading)


def comments(tokens):
    for position, token in enumerate(tokens):
        if token.kind == "comment" and not license_header(tokens, position):
            yield token.line, "comment", token.text.splitlines()[0].strip()


def sql(tokens):
    for token in tokens:
        if token.kind == "string" and any(pattern.search(token.value) for pattern in SQL):
            yield token.line, "sql", " ".join(token.value.split())[:80]


def attribute(tokens, index):
    if tokens[index].text != "#" or index + 1 >= len(tokens):
        return None
    opening = index + 1
    if tokens[opening].text == "!":
        opening += 1
    if opening >= len(tokens) or tokens[opening].text != "[":
        return None
    end = matching(tokens, opening)
    return tokens[opening + 1:end], end + 1


def path_of(content):
    names = []
    for position, token in enumerate(content):
        if token.kind == "ident":
            names.append(token.text)
        elif token.text != ":":
            return names, content[position:]
    return names, []


def requires_test(predicate):
    if len(predicate) == 1:
        return predicate[0].kind == "ident" and predicate[0].text == "test"
    if len(predicate) >= 3 and predicate[0].text == "all" and predicate[1].text == "(":
        depth = 0
        argument = []
        for token in predicate[2:-1]:
            if token.text == "," and depth == 0:
                if requires_test(argument):
                    return True
                argument = []
                continue
            depth += token.text in ("(", "[")
            depth -= token.text in (")", "]")
            argument.append(token)
        return requires_test(argument)
    return False


def inline_tests(tokens):
    tokens = code(tokens)
    index = 0
    while index < len(tokens):
        parsed = attribute(tokens, index)
        if parsed is None:
            index += 1
            continue
        content, after = parsed
        names, arguments = path_of(content)
        if names and names[-1] in TEST_ATTRIBUTES and (not arguments or arguments[0].text == "("):
            yield tokens[index].line, "inline-test", f"#[{''.join(token.text for token in content)}] in src"
        elif names == ["cfg"] and len(content) > 2 and requires_test(content[2:-1]):
            line = tokens[index].line
            redirected = False
            cursor = after
            while (following := attribute(tokens, cursor)) is not None:
                inner, cursor = following
                if path_of(inner)[0] == ["path"] and len(inner) == 3 and inner[2].kind == "string" and "tests/" in inner[2].value:
                    redirected = True
            if cursor < len(tokens) and tokens[cursor].text == "pub":
                cursor += 1
                if cursor < len(tokens) and tokens[cursor].text == "(":
                    cursor = matching(tokens, cursor) + 1
            if cursor + 2 < len(tokens) and tokens[cursor].text == "mod":
                ending = tokens[cursor + 2].text
                if ending == "{":
                    yield line, "inline-test", f"#[cfg(test)] mod {tokens[cursor + 1].text} {{ in src"
                elif ending == ";" and not redirected:
                    yield line, "inline-test", f"#[cfg(test)] mod {tokens[cursor + 1].text}; without #[path] into tests/"
        index += 1


def first_argument(tokens, opening):
    argument = tokens[opening + 1:matching(tokens, opening)]
    if argument and argument[-1].text == ",":
        argument = argument[:-1]
    if argument and argument[0].text == "&":
        argument = argument[1:]
    return argument


def messages(tokens):
    found = {}
    for index in range(len(tokens) - 6):
        name, colon, reference, kind, equals, literal = tokens[index + 1:index + 7]
        if tokens[index].text in ("const", "static") and (colon.text, reference.text, kind.text, equals.text) == (":", "&", "str", "=") and literal.kind == "string":
            found[name.text] = literal
    return found


def invariants(tokens):
    tokens = code(tokens)
    constants = messages(tokens)
    for index, token in enumerate(tokens):
        if token.kind != "ident" or index + 1 >= len(tokens):
            continue
        if token.text == "expect" and index > 0 and tokens[index - 1].text == "." and tokens[index + 1].text == "(":
            argument = first_argument(tokens, index + 1)
            if len(argument) == 1 and argument[0].kind == "ident" and argument[0].text in constants:
                argument = [constants[argument[0].text]]
            if len(argument) == 1 and argument[0].kind == "string" and not argument[0].value.startswith("invariant: "):
                yield token.line, "invariant", f'.expect("{argument[0].value[:70]}") does not start with "invariant: "'
            elif len(argument) > 1 and argument[0].text in ("format", "concat") and argument[1].text == "!":
                yield token.line, "invariant", ".expect(...) takes a literal \"invariant: …\" message"
        elif token.text == "unreachable" and tokens[index + 1].text == "!" and index + 2 < len(tokens) and tokens[index + 2].text in ("(", "[", "{"):
            argument = first_argument(tokens, index + 2)
            if not argument or argument[0].kind != "string" or not argument[0].value.strip():
                yield token.line, "invariant", "unreachable! without a message naming the invariant"


def rust_files(root):
    for path in sorted((root / "crates").rglob("*.rs")):
        relative = path.relative_to(root)
        if "target" not in relative.parts:
            yield relative


def in_src(relative):
    return len(relative.parts) > 3 and relative.parts[0] == "crates" and relative.parts[2] == "src"


def production(relative):
    return in_src(relative) and relative.parts[1] not in TEST_CRATES


def check(relative, source):
    try:
        tokens = tokenize(source)
        found = list(comments(tokens))
        if in_src(relative):
            found += list(sql(tokens)) + list(inline_tests(tokens))
        if production(relative):
            found += list(invariants(tokens))
    except LexError as error:
        found = [(error.line, "lex", str(error))]
    return sorted(Violation(relative.as_posix(), line, rule, message) for line, rule, message in found)


def scan(root):
    violations = []
    for relative in rust_files(root):
        violations += check(relative, (root / relative).read_text(encoding="utf-8"))
    return violations


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rule", action="append", choices=("comment", "sql", "inline-test", "invariant", "lex"))
    args = parser.parse_args()
    violations = [violation for violation in scan(ROOT) if args.rule is None or violation.rule in args.rule]
    for violation in violations:
        print(f"{violation.path}:{violation.line}: {violation.rule}: {violation.message}")
    if violations:
        print(f"{len(violations)} repository rule violations; see AGENTS.md", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
