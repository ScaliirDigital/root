import unittest

from main import message


class MessageTest(unittest.TestCase):
    def test_message(self) -> None:
        self.assertEqual(message(), "Hello from Python & Bazel!")


if __name__ == "__main__":
    unittest.main()
