//go:build !linux

package main

import (
	"context"
	"errors"
)

func guestTaskEvents(context.Context, string) (<-chan struct{}, error) {
	return nil, errors.New("Proxmox task events require a Linux collector")
}

func validateTaskDirectory(string) error {
	return errors.New("Proxmox task events require a Linux collector")
}
