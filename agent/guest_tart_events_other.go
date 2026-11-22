//go:build !darwin

package main

import (
	"context"
	"errors"
)

func tartGuestEvents(context.Context, string) (<-chan struct{}, error) {
	return nil, errors.New("Tart events require a macOS collector")
}
