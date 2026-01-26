#pragma once

#include <vector>
#include <stdexcept>

/*
 Store a flat list of items.

 Exposts an 'id' which is the 1-based index.

 IDs with a value of 0 are considered 'null'.
*/
template<typename T>
struct LookupTable {
    std::vector<T> items;

    T& operator[](size_t index) {
        auto actual_index = index - 1;
        if (actual_index < 0) {
            throw std::runtime_error("LookupTable index out of bounds");
        }
        if (actual_index >= items.size()) {
            throw std::runtime_error("LookupTable index out of bounds");
        }
        return items[actual_index];
    }

    size_t size() const {
        return items.size();
    }

    /*
      .add()

      Add a new item to the table.
    */
    void add(const T& item) {
        items.push_back(item);
    }

    /*
      .last_id()

      Return the ID of the last item in the table.
    */
    size_t last_id() const {
        return items.size();
    }
};
