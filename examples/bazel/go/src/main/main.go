package main

import (
	"fmt"

	"example.com/basic/src/lib"
	"github.com/google/uuid"
)

func main() {
	fmt.Println(lib.Message())
	fmt.Printf("UUID: %s\n", uuid.NewString())
}
