#include <cstdlib>

struct Node {
    int value;
    Node* next;
};

int main() {
    Node* a = new Node{1, nullptr};
    a->value = 5;
    std::abort();
}
