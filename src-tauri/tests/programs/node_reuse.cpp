struct Node {
    int value;
    Node* next;
};

int main() {
    Node* prev = nullptr;
    for (int i = 0; i < 300; i++) {
        Node* n = new Node{i, nullptr};
        prev = n;
        delete n;
    }
    return prev == nullptr;
}
