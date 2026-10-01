struct Node {
    int value;
    Node* next;
};

int main() {
    Node* a = new Node{10, nullptr};
    Node* b = new Node{20, nullptr};

    a->next = b;
    b->next = a;

    delete b;
    delete a;
}
