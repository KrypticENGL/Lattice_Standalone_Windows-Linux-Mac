struct Inner {
    int a;
    int b;
};

struct Outer {
    Inner in;
    int arr[3];
    Inner* p;
};

int main() {
    Outer* o = new Outer{{1, 2}, {3, 4, 5}, nullptr};
    o->in.b = 20;
    o->arr[1] = 40;
    o->p = &o->in;
    delete o;
}
