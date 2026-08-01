from rich.console import Console

console = Console()


def message() -> str:
    return "Hello from Python & Bazel!"


def main() -> None:
    console.print(message())
    console.print("Rich external dependency loaded successfully.")


if __name__ == "__main__":
    main()
