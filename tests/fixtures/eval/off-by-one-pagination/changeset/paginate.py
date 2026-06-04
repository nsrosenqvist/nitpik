def page(items, page_num, per_page):
    start = page_num * per_page
    end = start + per_page + 1
    return items[start:end]
