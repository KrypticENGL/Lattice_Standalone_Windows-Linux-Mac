struct Node {
    int value;
    Node* next;
};

int square(int n) {
    int result = n * n;
    return result;
}

void link(Node* a, Node* b) {
    a->next = b;
}

int main() {
    Node* first = new Node{1, nullptr};
    Node second{2, nullptr};
    int total = 0;
    for (int i = 1; i <= 3; i++) {
        int sq = square(i);
        total += sq;
    }
    link(first, &second);
    {
        int inner = total;
        inner++;
    }
    delete first;
    return total == 14 ? 0 : 1;
}
