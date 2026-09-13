"""Detect unittest methods hidden by a later definition, without importing extensions."""
import ast
from pathlib import Path
import sys


def main(paths):
    errors = []
    for directory in paths:
        for path in Path(directory).rglob("test*.py"):
            for cls in ast.walk(ast.parse(path.read_text())):
                if not isinstance(cls, ast.ClassDef):
                    continue
                seen = {}
                for method in cls.body:
                    if not isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef)):
                        continue
                    if not method.name.startswith("test"):
                        continue
                    if method.name in seen:
                        errors.append(f"{path}:{method.lineno}: {cls.name}.{method.name} shadows line {seen[method.name]}")
                    seen[method.name] = method.lineno
    for error in errors:
        print(error)
    return bool(errors)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
