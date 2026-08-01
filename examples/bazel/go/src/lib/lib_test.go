package lib

import "testing"

func TestMessage(t *testing.T) {
	got := Message()
	want := "Hello from Go & Bazel!"

	if got != want {
		t.Fatalf("got %q, want %q", got, want)
	}
}
