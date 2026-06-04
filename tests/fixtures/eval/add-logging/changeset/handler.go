package main

import "log"

func process(items []string) int {
	n := len(items)
	log.Printf("processed %d items", n)
	return n
}
