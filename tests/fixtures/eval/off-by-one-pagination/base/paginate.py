def page(items, page_num, per_page):
    start = page_num * per_page
    end = start + per_page
    return items[start:end]
