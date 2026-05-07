#include <App.h>

#include <iostream>

int main() {
  uWS::App()
      .get("/", [](auto *res, auto * /*req*/) {
        res->end("Hello from uWebSockets + Bazel\n");
      })
      .listen(9001, [](auto *listen_socket) {
        if (listen_socket) {
          std::cout << "Listening on http://localhost:9001\n";
        } else {
          std::cerr << "Failed to listen on port 9001\n";
        }
      })
      .run();

  return 0;
}
