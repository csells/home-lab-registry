//go:build linux

package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestTaskWatchClosesWhenItsDirectoryMoves(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "tasks")
	if err := os.Mkdir(path, 0700); err != nil {
		t.Fatal(err)
	}
	wake, err := guestTaskEvents(t.Context(), path)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(path, path+"-moved"); err != nil {
		t.Fatal(err)
	}
	select {
	case _, open := <-wake:
		if open {
			t.Fatal("moving the watched directory should close the event source")
		}
	case <-time.After(time.Second):
		t.Fatal("watch did not close after its directory moved")
	}
}
