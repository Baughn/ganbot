// Gallery tag filtering functionality
document.addEventListener('DOMContentLoaded', () => {
    const filterButtons = document.querySelectorAll('.tag-filter');
    const modelRows = document.querySelectorAll('tbody tr[data-tags]');

    // Apply initial filter based on active button
    const activeButton = document.querySelector('.tag-filter.active');
    if (activeButton) {
        applyFilter(activeButton.dataset.tag);
    }

    function applyFilter(selectedTag) {
        modelRows.forEach(row => {
            const rowTags = row.dataset.tags.split(',');

            if (selectedTag === 'all' || rowTags.includes(selectedTag)) {
                row.style.display = '';
            } else {
                row.style.display = 'none';
            }
        });
    }

    filterButtons.forEach(button => {
        button.addEventListener('click', () => {
            const selectedTag = button.dataset.tag;

            // Update active button
            filterButtons.forEach(btn => btn.classList.remove('active'));
            button.classList.add('active');

            // Filter rows
            applyFilter(selectedTag);
        });
    });
});
