#include <iostream>

int main() {
    std::cout << "spinning" << std::endl;
    volatile unsigned long long n = 0;
    for (;;) {
        n = n + 1;
    }
}
