#include <cstdio>

struct Node {
    int value;
    Node* next;
};

int main() {
    Node* a = new Node{1, nullptr};
    Node* b = new Node{2, nullptr};
    a->next = b;
    std::printf("%d -> %d\n", a->value, a->next->value);
    delete b;
    delete a;
    return 3;
}
